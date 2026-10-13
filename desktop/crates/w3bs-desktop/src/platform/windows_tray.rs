//! `w3bs desktop tray` on Windows: a notification-area icon speaking the native-control protocol.
//!
//! It holds the native-control subscription (like the Linux tray and `w3bs desktop control`), so
//! closing it while access is enabled disables the layer. State is stated in words in the
//! tooltip and the first menu line, never by icon alone.
//!
//! * Right click (or keyboard selection): the menu. Left click disables when enabled and never
//!   enables; enabling always goes through the menu.
//! * The emergency-stop chord (`Win+Alt+Esc`, or the fallback) is registered here with
//!   `RegisterHotKey` unless the broker's native adapter already holds it, in which case the
//!   broker reacts to it directly and the menu just names it.
//! * The notification area belongs to Explorer. A watcher thread waits on the Explorer process
//!   that owns the taskbar (`Shell_TrayWnd`); when it exits, access is disabled at once, because
//!   the human can no longer see the switch. When Explorer comes back it broadcasts
//!   `TaskbarCreated` and the icon is added again (access stays disabled until the human enables).
//!   A failed icon update (checked every 2 s) is treated the same way.
//!
//! The window is a hidden top-level window (class `W3bsDesktopTray`), not a message-only one,
//! because `TaskbarCreated` is a broadcast and message-only windows receive no broadcasts.

use crate::paths;
use adl_schema::rpc::{Request, Response};
use adl_schema::SCHEMA_VERSION;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_REFRESH: u32 = WM_APP + 2;
const WM_BROKER_LOST: u32 = WM_APP + 3;
const ICON_ID: u32 = 1;
const HEALTH_TIMER: usize = 1;

pub const ID_ENABLE: usize = 1;
pub const ID_CONFIRM: usize = 2;
pub const ID_DISABLE: usize = 3;
pub const ID_STOP: usize = 4;
pub const ID_QUIT: usize = 5;
/// Approval i: approve = ID_APPROVAL + 2i, deny = ID_APPROVAL + 2i + 1.
pub const ID_APPROVAL: usize = 100;

#[derive(Clone, Debug)]
enum Act {
    Enable,
    ConfirmPending,
    Disable(&'static str),
    Stop,
    Emergency,
    Decide(String, bool),
    Quit,
}

#[derive(Default)]
struct View {
    state: String,
    reason: Option<String>,
    lease: Option<(String, String)>,
    approvals: Vec<(String, String)>,
    /// Approval ids in the order the last menu showed them.
    menu_approvals: Vec<String>,
    pending_code: Option<String>,
    note: Option<String>,
    shortcut: Option<String>,
    balloon: Option<(String, String)>,
}

impl View {
    fn state_text(&self) -> String {
        let s = match self.state.as_str() {
            "" => "Connecting",
            "enabled" => "Enabled",
            "disabled" => "Disabled",
            "enabling" => "Enabling",
            "stopping" => "Stopping",
            "blocked" => "Blocked",
            other => other,
        };
        match &self.reason {
            Some(r) => format!("{s} ({r})"),
            None => s.to_string(),
        }
    }
    fn enabled(&self) -> bool {
        self.state == "enabled"
    }
    fn attention(&self) -> bool {
        self.lease.is_some() || !self.approvals.is_empty() || self.pending_code.is_some()
    }
}

struct Ctx {
    view: Arc<Mutex<View>>,
    acts: mpsc::Sender<Act>,
    taskbar_created: u32,
    hwnd: AtomicIsize,
    icon_shown: AtomicBool,
    own_hotkey: AtomicBool,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static BROKER_LOST: AtomicBool = AtomicBool::new(false);

fn ctx() -> &'static Ctx {
    CTX.get().expect("tray context")
}

fn view() -> std::sync::MutexGuard<'static, View> {
    ctx().view.lock().unwrap_or_else(|e| e.into_inner())
}

fn post(msg: u32) {
    let h = HWND(ctx().hwnd.load(Ordering::SeqCst) as *mut _);
    if !h.0.is_null() {
        // SAFETY: posting to our own window.
        unsafe {
            let _ = PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for u in s.encode_utf16() {
        if n + 1 >= dst.len() {
            break;
        }
        dst[n] = u;
        n += 1;
    }
    dst[n] = 0;
}

fn tooltip(v: &View) -> String {
    let mut t = format!("Agent access: {}", v.state_text());
    if let Some((agent, _)) = &v.lease {
        t += &format!("\nAgent {agent} is controlling input");
    }
    if !v.approvals.is_empty() {
        t += &format!("\n{} approval(s) waiting", v.approvals.len());
    }
    t
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let v = view();
    let stock = if v.attention() {
        IDI_WARNING
    } else if v.enabled() {
        IDI_INFORMATION
    } else {
        IDI_APPLICATION
    };
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        // SAFETY: stock icon, never freed.
        hIcon: unsafe { LoadIconW(None, stock) }.unwrap_or_default(),
        ..Default::default()
    };
    copy_wide(&mut nid.szTip, &tooltip(&v));
    nid
}

/// Add (or re-add) the icon. Returns whether Shell_NotifyIcon succeeded.
fn add_icon(hwnd: HWND) -> bool {
    let mut nid = icon_data(hwnd);
    // SAFETY: valid NOTIFYICONDATAW for our window.
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool();
    if ok {
        nid.Anonymous.uVersion = 4; // NOTIFYICON_VERSION_4
        unsafe {
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
    }
    ctx().icon_shown.store(ok, Ordering::SeqCst);
    ok
}

fn refresh_icon(hwnd: HWND) -> bool {
    let mut nid = icon_data(hwnd);
    if let Some((title, text)) = view().balloon.take() {
        nid.uFlags |= NIF_INFO;
        copy_wide(&mut nid.szInfoTitle, &title);
        copy_wide(&mut nid.szInfo, &text);
        nid.dwInfoFlags = NIIF_INFO;
    }
    // SAFETY: valid NOTIFYICONDATAW for our window.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) }.as_bool()
}

fn remove_icon(hwnd: HWND) {
    let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: ICON_ID, ..Default::default() };
    // SAFETY: valid NOTIFYICONDATAW.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
    ctx().icon_shown.store(false, Ordering::SeqCst);
}

/// Menu entries: (id or 0 for a disabled label / usize::MAX for a separator, text).
fn menu_items() -> Vec<(usize, String)> {
    let mut v = view();
    let mut m = vec![(0, format!("Status: {}", v.state_text()))];
    if let Some((agent, task)) = &v.lease {
        m.push((0, format!("Active: {agent}, task {task}")));
    }
    if let Some(note) = &v.note {
        m.push((0, note.clone()));
    }
    m.push((usize::MAX, String::new()));
    if v.enabled() {
        m.push((ID_DISABLE, "Disable agent access".into()));
    } else {
        m.push((ID_ENABLE, "Enable agent access".into()));
    }
    if let Some(code) = &v.pending_code {
        m.push((ID_CONFIRM, format!("Confirm the enable request from a terminal (code {code})")));
    }
    v.menu_approvals = v.approvals.iter().map(|(id, _)| id.clone()).collect();
    if !v.approvals.is_empty() {
        m.push((usize::MAX, String::new()));
        for (i, (_, summary)) in v.approvals.iter().enumerate() {
            m.push((0, format!("Approval: {summary}")));
            m.push((ID_APPROVAL + 2 * i, "    Approve".into()));
            m.push((ID_APPROVAL + 2 * i + 1, "    Deny".into()));
        }
    }
    m.push((usize::MAX, String::new()));
    m.push((ID_STOP, "Stop all agent activity".into()));
    m.push((ID_QUIT, "Quit and disable".into()));
    if let Some(s) = &v.shortcut {
        m.push((0, format!("Emergency stop: {s}")));
    }
    m
}

fn show_menu(hwnd: HWND) {
    let items = menu_items();
    // Logged for diagnostics, with any enable code masked.
    let shown: Vec<String> = items.iter().filter(|(id, _)| *id != usize::MAX).map(|(id, t)| if *id == ID_CONFIRM { "Confirm the enable request from a terminal (code ******)".into() } else { t.trim().to_string() }).collect();
    eprintln!("w3bs-desktop tray: menu: {}", shown.join(" | "));
    // SAFETY: menu created, shown modally and destroyed on this (the window's) thread.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (id, text) in &items {
            if *id == usize::MAX {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            } else {
                let flags = if *id == 0 { MF_STRING | MF_GRAYED } else { MF_STRING };
                let _ = AppendMenuW(menu, flags, *id, &HSTRING::from(text.as_str()));
            }
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn command(id: usize) {
    let c = ctx();
    let act = match id {
        ID_ENABLE => Some(Act::Enable),
        ID_CONFIRM => Some(Act::ConfirmPending),
        ID_DISABLE => Some(Act::Disable("disabled from the tray")),
        ID_STOP => Some(Act::Stop),
        ID_QUIT => Some(Act::Quit),
        n if n >= ID_APPROVAL => {
            let i = (n - ID_APPROVAL) / 2;
            view().menu_approvals.get(i).cloned().map(|a| Act::Decide(a, (n - ID_APPROVAL).is_multiple_of(2)))
        }
        _ => None,
    };
    if let Some(a) = act {
        let _ = c.acts.send(a);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let c = ctx();
    match msg {
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: the event is LOWORD(lParam).
            match (lp.0 as u32) & 0xFFFF {
                WM_CONTEXTMENU | WM_RBUTTONUP => show_menu(hwnd),
                x if x == NIN_SELECT || x == WM_LBUTTONUP => {
                    if view().enabled() {
                        let _ = c.acts.send(Act::Disable("disabled by clicking the tray icon"));
                    }
                }
                x if x == NIN_SELECT + 1 => show_menu(hwnd), // NIN_KEYSELECT
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            command(wp.0 & 0xFFFF);
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == adl_windows::HOTKEY_ID => {
            let _ = c.acts.send(Act::Emergency);
            LRESULT(0)
        }
        WM_REFRESH => {
            if c.icon_shown.load(Ordering::SeqCst) && !refresh_icon(hwnd) {
                // The notification area is gone (Explorer crashed or is restarting).
                c.icon_shown.store(false, Ordering::SeqCst);
                eprintln!("w3bs-desktop tray: notification area unavailable; disabling access until the icon is back");
                let _ = c.acts.send(Act::Disable("the notification area went away"));
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == HEALTH_TIMER => {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_REFRESH, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_BROKER_LOST => {
            BROKER_LOST.store(true, Ordering::SeqCst);
            remove_icon(hwnd);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_ENDSESSION | WM_CLOSE => {
            remove_icon(hwnd);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_icon(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m == c.taskbar_created && m != 0 => {
            let ok = add_icon(hwnd);
            eprintln!("w3bs-desktop tray: TaskbarCreated (Explorer restarted); icon re-added: {ok}");
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Agent id -> registered name, over a second control connection.
/// Wait on the Explorer process that owns the taskbar; disable access whenever it exits.
fn watch_explorer(acts: mpsc::Sender<Act>) {
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE};
    loop {
        // SAFETY: plain window lookup and a process handle owned by `Owned`.
        let owner = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok().map(adl_windows::win::window_pid).filter(|p| *p != 0);
        let Some(pid) = owner else {
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        };
        let Ok(h) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
            eprintln!("w3bs-desktop tray: cannot watch Explorer (pid {pid}); relying on icon updates");
            return;
        };
        let h = adl_windows::win::Owned(h);
        unsafe { WaitForSingleObject(h.0, INFINITE) };
        eprintln!("w3bs-desktop tray: Explorer (pid {pid}, owner of the notification area) exited; disabling access");
        ctx().icon_shown.store(false, Ordering::SeqCst);
        if acts.send(Act::Disable("the notification area (Explorer) went away")).is_err() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn agent_name(id: &str) -> String {
    let lookup = || -> Option<String> {
        let mut c = crate::client::Client::connect(&paths::control_socket()).ok()?;
        let v = c.call("agents.list", json!({})).ok()?;
        v.as_array()?.iter().find(|a| a["id"] == id)?["name"].as_str().map(Into::into)
    };
    lookup().unwrap_or_else(|| id.to_string())
}

/// The emergency chord the broker's adapter registered, from its capability features.
fn broker_hotkey() -> Option<String> {
    let mut c = crate::client::Client::connect(&paths::control_socket()).ok()?;
    let caps = c.call("capabilities", json!({})).ok()?;
    caps.as_array()?.iter().flat_map(|c| c["features"].as_array().cloned().unwrap_or_default()).find_map(|f| f.as_str()?.strip_prefix("emergency_hotkey:").map(str::to_owned))
}

pub fn run() -> std::io::Result<()> {
    adl_windows::win::init_dpi_awareness();
    let ep = paths::control_socket();
    let stream = crate::transport::connect(&ep).map_err(|e| std::io::Error::new(e.kind(), format!("{ep}: {e}; start the broker with `w3bs desktop daemon`")))?;
    let mut writer = stream.try_clone_box()?;
    let reader = BufReader::new(stream);
    let (act_tx, act_rx) = mpsc::channel::<Act>();
    let (resp_tx, resp_rx) = mpsc::channel::<Response>();
    let view_arc = Arc::new(Mutex::new(View::default()));
    // SAFETY: registering a window message name.
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if CTX.set(Ctx { view: view_arc.clone(), acts: act_tx.clone(), taskbar_created, hwnd: AtomicIsize::new(0), icon_shown: AtomicBool::new(false), own_hotkey: AtomicBool::new(false) }).is_err() {
        return Err(std::io::Error::other("tray already running in this process"));
    }

    // Broker events -> view.
    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(event) = v.get("event").and_then(Value::as_str) else {
                if let Ok(r) = serde_json::from_value::<Response>(v) {
                    let _ = resp_tx.send(r);
                }
                continue;
            };
            let data = &v["data"];
            let who = data["agent"].as_str().map(agent_name).unwrap_or_default();
            {
                let mut t = view();
                match event {
                    "state" => {
                        t.state = data["state"].as_str().unwrap_or("").into();
                        t.reason = data["reason"].as_str().map(Into::into);
                        if t.state != "enabling" {
                            t.pending_code = None;
                        }
                        if t.state != "enabled" {
                            t.lease = None;
                            t.approvals.clear();
                        }
                        eprintln!("w3bs-desktop tray: state {}", t.state_text());
                    }
                    "enable_requested" => {
                        t.pending_code = data["code"].as_str().map(Into::into);
                        t.balloon = Some(("Enable requested".into(), "A terminal asked to enable agent access. Confirm from the tray menu, or ignore it to refuse.".into()));
                    }
                    "lease" => t.lease = data.as_object().map(|l| (who.clone(), l["task"].as_str().unwrap_or("").into())),
                    "approval_requested" => {
                        let summary = format!("{who} wants: {}", data["summary"].as_str().unwrap_or(""));
                        t.balloon = Some(("Approval needed".into(), format!("{summary}. Agent input is paused; approve or deny from the tray menu.")));
                        t.approvals.push((data["id"].as_str().unwrap_or("").into(), summary));
                    }
                    "approval_decided" => t.approvals.retain(|(id, _)| Some(id.as_str()) != data["id"].as_str()),
                    _ => {}
                }
            }
            post(WM_REFRESH);
        }
        eprintln!("w3bs-desktop tray: broker connection closed");
        post(WM_BROKER_LOST);
    });

    // Actions -> broker calls, off the UI thread (calls wait for responses).
    let action_thread = std::thread::spawn(move || {
        let mut next = 0u64;
        let mut call = |method: &str, params: Value| -> Result<Value, String> {
            next += 1;
            let req = Request { v: SCHEMA_VERSION, id: next.to_string(), method: method.into(), params };
            writeln!(writer, "{}", serde_json::to_string(&req).unwrap()).and_then(|_| writer.flush()).map_err(|e| e.to_string())?;
            loop {
                let r = resp_rx.recv().map_err(|e| e.to_string())?;
                if r.id == next.to_string() {
                    return match r.error {
                        Some(e) => Err(e.message),
                        None => Ok(r.result.unwrap_or(Value::Null)),
                    };
                }
            }
        };
        if let Err(e) = call("subscribe", json!({"native_control": true})) {
            eprintln!("w3bs-desktop tray: subscribe failed: {e}");
            post(WM_BROKER_LOST);
            return;
        }
        for act in act_rx {
            eprintln!("w3bs-desktop tray: action {act:?}");
            let r = match &act {
                Act::Enable => call("enable.request", json!({})).and_then(|c| match c["code"].as_str() {
                    Some(code) => call("enable.confirm", json!({"code": code})),
                    None => Err("the broker did not hand the code to this control".into()),
                }),
                Act::ConfirmPending => match view().pending_code.take() {
                    Some(code) => call("enable.confirm", json!({"code": code})),
                    None => Err("no pending enable request".into()),
                },
                Act::Disable(why) => call("disable", json!({"reason": why})),
                Act::Stop => call("stop", json!({"reason": "stopped from the tray"})),
                Act::Emergency => call("session.event", json!({"event": "emergency_stop"})),
                Act::Decide(id, ok) => call("approvals.decide", json!({"approval": id, "approve": ok})),
                Act::Quit => {
                    let _ = call("disable", json!({"reason": "tray closed"}));
                    post(WM_CLOSE);
                    return;
                }
            };
            let note = r.err().map(|e| format!("Last action failed: {e}"));
            if let Some(n) = &note {
                eprintln!("w3bs-desktop tray: {n}");
            }
            view().note = note;
            post(WM_REFRESH);
        }
    });

    // SAFETY: window class, window and message loop on this thread.
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class = w!("W3bsDesktopTray");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        CreateWindowExW(WS_EX_TOOLWINDOW, class, w!("W3BS Agentic Desktop Layer tray"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None).map_err(|e| std::io::Error::other(format!("cannot create the tray window: {e}")))?
    };
    ctx().hwnd.store(hwnd.0 as isize, Ordering::SeqCst);

    // The icon must be visible before this process counts as the human's switch; at login the
    // notification area may take a moment to appear.
    let t0 = std::time::Instant::now();
    while !add_icon(hwnd) {
        if t0.elapsed() > std::time::Duration::from_secs(30) {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(std::io::Error::other("Shell_NotifyIcon failed: no notification area (is Explorer running?); use `w3bs desktop control`"));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    eprintln!("w3bs-desktop tray: notification icon added (Shell_NotifyIcon ok), window {}", adl_windows::map::window_id(hwnd.0 as isize));

    // The broker's native adapter normally holds the chord and reacts to it itself; only when
    // it does not (mock adapter, chord taken) does the tray register one.
    match broker_hotkey() {
        Some(desc) => {
            eprintln!("w3bs-desktop tray: emergency stop {desc} is held by the broker's adapter");
            view().shortcut = Some(desc);
        }
        None => match adl_windows::register_hotkey(hwnd) {
            Some(desc) => {
                ctx().own_hotkey.store(true, Ordering::SeqCst);
                view().shortcut = Some(desc.to_string());
                eprintln!("w3bs-desktop tray: emergency stop {desc} registered");
            }
            None => eprintln!("w3bs-desktop tray: no emergency-stop chord available; use the menu"),
        },
    }
    {
        let tx = act_tx.clone();
        std::thread::spawn(move || watch_explorer(tx));
    }
    // SAFETY: timer on our own window.
    unsafe {
        SetTimer(Some(hwnd), HEALTH_TIMER, 2000, None);
    }
    post(WM_REFRESH);

    let mut msg = MSG::default();
    // SAFETY: standard message loop.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // The action thread ends on Quit (it posted WM_CLOSE); on a lost broker it is left blocked.
    if action_thread.is_finished() || !BROKER_LOST.load(Ordering::SeqCst) {
        return Ok(());
    }
    drop((act_tx, view_arc));
    Err(std::io::Error::other("the broker connection closed"))
}
