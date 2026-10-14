//! macOS: the native backend (`adl-macos`: Core Graphics events, CGWindowList, Accessibility)
//! and the menu-bar native control.

use crate::AdapterKind;
use adl_broker::Adapter;
use adl_schema::*;
use std::sync::Arc;

pub fn make_adapter(kind: AdapterKind) -> Result<Arc<dyn Adapter>> {
    match kind {
        AdapterKind::Mock => Ok(Arc::new(adl_broker::mock::MockDesktop::new())),
        AdapterKind::Auto | AdapterKind::Native => Ok(Arc::new(adl_macos::MacAdapter::new())),
        AdapterKind::Vela | AdapterKind::Portal => Err(AdlError::unsupported("vela and portal are Linux backends; on macOS use auto or native")),
    }
}

pub fn run_tray() -> std::io::Result<()> {
    menubar::run()
}

/// Permission state for `doctor`, optionally showing the system prompts first.
pub fn permission_checks(prompt: bool) -> Vec<(&'static str, bool, String)> {
    use adl_macos::permissions::{self, ACCESSIBILITY_PANE, SCREEN_RECORDING_PANE};
    let p = if prompt { permissions::request() } else { permissions::check() };
    let state = |ok: bool| if ok { "allowed" } else { "not allowed" };
    vec![
        ("macos_accessibility", p.accessibility, format!("Accessibility (focus, inspect): {}; {ACCESSIBILITY_PANE}", state(p.accessibility))),
        ("macos_post_events", p.post_events, format!("posting input events: {}; granted in {ACCESSIBILITY_PANE}", state(p.post_events))),
        ("macos_screen_recording", p.screen_capture, format!("Screen Recording (captures, window titles): {}; {SCREEN_RECORDING_PANE}", state(p.screen_capture))),
        ("macos_input_monitoring", true, "not needed: the layer never reads keys (physical input is detected from HID idle time)".into()),
    ]
}

/// The menu-bar control: an `NSStatusItem` in an accessory (no Dock icon) application.
///
/// It speaks the same native-control protocol as `control` and the Linux tray: it holds the
/// `native_control` subscription, so quitting or crashing disables access. AppKit runs on the
/// main thread; broker I/O runs on two background threads (events in, actions out) and reaches
/// the UI through a channel that a main-thread timer drains. State is shown in words in the
/// menu-bar title, never by colour alone.
mod menubar {
    use crate::paths;
    use adl_schema::rpc::{Request, Response};
    use adl_schema::SCHEMA_VERSION;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSScreen, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
    use objc2_foundation::{NSObjectProtocol, NSString, NSTimer};
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;

    #[derive(Clone, Debug)]
    enum Act {
        Enable,
        ConfirmPending,
        Disable,
        Stop,
        Emergency,
        Decide(String, bool),
        Permissions,
        OpenPanel,
        Shortcut(String),
        Quit,
    }

    enum Ui {
        Event(Value),
        Note(Option<String>),
    }

    #[derive(Default)]
    struct View {
        state: String,
        reason: Option<String>,
        lease: Option<(String, String)>,
        approvals: Vec<(String, String)>,
        pending_code: Option<String>,
        note: Option<String>,
        shortcut: Option<String>,
    }

    impl View {
        fn state_word(&self) -> &str {
            match self.state.as_str() {
                "" => "Connecting",
                "enabled" => "Enabled",
                "disabled" => "Disabled",
                "enabling" => "Enabling",
                "stopping" => "Stopping",
                "blocked" => "Blocked",
                other => other,
            }
        }
        fn state_text(&self) -> String {
            match &self.reason {
                Some(r) => format!("{} ({r})", self.state_word()),
                None => self.state_word().to_string(),
            }
        }
        fn enabled(&self) -> bool {
            self.state == "enabled"
        }
        fn apply(&mut self, v: &Value) {
            let data = &v["data"];
            match v["event"].as_str() {
                Some("state") => {
                    self.state = data["state"].as_str().unwrap_or("").into();
                    self.reason = data["reason"].as_str().map(Into::into);
                    if self.state != "enabling" {
                        self.pending_code = None;
                    }
                    if self.state != "enabled" {
                        self.lease = None;
                        self.approvals.clear();
                    }
                }
                Some("enable_requested") => self.pending_code = data["code"].as_str().map(Into::into),
                Some("lease") => self.lease = data.as_object().map(|l| (v["agent_name"].as_str().unwrap_or("").into(), l["task"].as_str().unwrap_or("").into())),
                Some("approval_requested") => self.approvals.push((data["id"].as_str().unwrap_or("").into(), format!("{} wants: {}", v["agent_name"].as_str().unwrap_or(""), data["summary"].as_str().unwrap_or("")))),
                Some("approval_decided") => self.approvals.retain(|(id, _)| Some(id.as_str()) != data["id"].as_str()),
                _ => {}
            }
        }
    }

    struct Ivars {
        item: Retained<NSStatusItem>,
        view: RefCell<View>,
        /// The action behind each enabled menu item, indexed by the item's tag.
        acts: RefCell<Vec<Act>>,
        ui_rx: mpsc::Receiver<Ui>,
        act_tx: mpsc::Sender<Act>,
        /// Ticks until the item's position is logged (it is placed once the run loop runs).
        log_frame_in: Cell<u32>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and `Target` does not implement Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "W3BSDesktopMenuTarget"]
        #[ivars = Ivars]
        struct Target;

        impl Target {
            #[unsafe(method(menuAction:))]
            fn menu_action(&self, sender: &NSMenuItem) {
                let act = self.ivars().acts.borrow().get(sender.tag() as usize).cloned();
                if let Some(a) = act {
                    let _ = self.ivars().act_tx.send(a);
                }
            }

            #[unsafe(method(tick:))]
            fn tick(&self, _timer: &NSTimer) {
                match self.ivars().log_frame_in.get() {
                    0 => {}
                    1 => {
                        self.ivars().log_frame_in.set(0);
                        self.log_frame();
                    }
                    n => self.ivars().log_frame_in.set(n - 1),
                }
                let mut changed = false;
                while let Ok(msg) = self.ivars().ui_rx.try_recv() {
                    let mut v = self.ivars().view.borrow_mut();
                    match msg {
                        Ui::Event(e) => v.apply(&e),
                        Ui::Note(n) => v.note = n,
                    }
                    changed = true;
                }
                if changed {
                    self.rebuild();
                }
            }
        }

        unsafe impl NSObjectProtocol for Target {}
    );

    impl Target {
        fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ivars);
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }

        fn add(&self, menu: &NSMenu, acts: &mut Vec<Act>, title: &str, act: Option<Act>) {
            let mtm = self.mtm();
            let action = act.as_ref().map(|_| sel!(menuAction:));
            // SAFETY: `menuAction:` is implemented by `Target`, which is set as the target.
            let it = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), action, &NSString::from_str("")) };
            match act {
                Some(a) => {
                    let target: &AnyObject = self.as_ref();
                    // SAFETY: the target outlives the menu (it lives for the whole process).
                    unsafe { it.setTarget(Some(target)) };
                    it.setTag(acts.len() as isize);
                    acts.push(a);
                }
                None => it.setEnabled(false),
            }
            menu.addItem(&it);
        }

        /// Log where the item is, in Quartz points (for test rigs and support: on macOS 26 the
        /// item is drawn by Control Center, so the process's own accessibility tree lacks it).
        fn log_frame(&self) {
            let mtm = self.mtm();
            let Some(win) = self.ivars().item.button(mtm).and_then(|b| b.window()) else { return };
            let f = win.frame();
            let h = NSScreen::screens(mtm).firstObject().map(|s| s.frame().size.height).unwrap_or(0.0);
            eprintln!("w3bs-desktop menu bar: status item at {},{} {}x{} (Quartz points)", f.origin.x, h - f.origin.y - f.size.height, f.size.width, f.size.height);
        }

        /// Rebuild the menu and the menu-bar title from the view.
        fn rebuild(&self) {
            let mtm = self.mtm();
            let iv = self.ivars();
            let v = iv.view.borrow();
            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            let mut acts = vec![];
            let sep = |m: &NSMenu| m.addItem(&NSMenuItem::separatorItem(mtm));
            self.add(&menu, &mut acts, "Open control panel…", Some(Act::OpenPanel));
            sep(&menu);
            self.add(&menu, &mut acts, &format!("Agent access: {}", v.state_text()), None);
            if let Some((agent, task)) = &v.lease {
                self.add(&menu, &mut acts, &format!("Active: {agent}, task {task}"), None);
            }
            if let Some(note) = &v.note {
                self.add(&menu, &mut acts, note, None);
            }
            sep(&menu);
            if v.enabled() {
                self.add(&menu, &mut acts, "Disable agent access", Some(Act::Disable));
            } else {
                self.add(&menu, &mut acts, "Enable agent access", Some(Act::Enable));
            }
            if let Some(code) = &v.pending_code {
                self.add(&menu, &mut acts, &format!("Confirm the enable request from a terminal (code {code})"), Some(Act::ConfirmPending));
            }
            if !v.approvals.is_empty() {
                sep(&menu);
                for (id, summary) in &v.approvals {
                    self.add(&menu, &mut acts, &format!("Approval: {summary}"), None);
                    self.add(&menu, &mut acts, "    Approve", Some(Act::Decide(id.clone(), true)));
                    self.add(&menu, &mut acts, "    Deny", Some(Act::Decide(id.clone(), false)));
                }
            }
            sep(&menu);
            self.add(&menu, &mut acts, "Stop all agent activity", Some(Act::Stop));
            self.add(&menu, &mut acts, "Check macOS permissions (shows the system prompts)", Some(Act::Permissions));
            self.add(&menu, &mut acts, "Quit and disable", Some(Act::Quit));
            if let Some(s) = &v.shortcut {
                self.add(&menu, &mut acts, &format!("Emergency stop: {s}"), None);
            }
            iv.item.setMenu(Some(&menu));
            if let Some(b) = iv.item.button(mtm) {
                let attention = if v.lease.is_some() || !v.approvals.is_empty() || v.pending_code.is_some() { " !" } else { "" };
                b.setTitle(&NSString::from_str(&format!("Agents: {}{attention}", v.state_word())));
                b.setToolTip(Some(&NSString::from_str(&format!("Agentic Desktop Layer: {}", v.state_text()))));
            }
            *iv.acts.borrow_mut() = acts;
        }
    }

    /// Agent id -> registered name, over a second control connection. Falls back to the id.
    fn agent_name(id: &str) -> String {
        let lookup = || -> Option<String> {
            let mut c = crate::client::Client::connect(&paths::control_socket()).ok()?;
            let v = c.call("agents.list", json!({})).ok()?;
            v.as_array()?.iter().find(|a| a["id"] == id)?["name"].as_str().map(Into::into)
        };
        lookup().unwrap_or_else(|| id.to_string())
    }

    pub fn run() -> std::io::Result<()> {
        let mtm = MainThreadMarker::new().ok_or_else(|| std::io::Error::other("the menu-bar control must run on the main thread"))?;
        let path = paths::control_socket();
        let stream = crate::transport::connect(&path).map_err(|e| std::io::Error::new(e.kind(), format!("{path}: {e}; start the broker with `w3bs desktop daemon`")))?;
        let mut writer = stream.try_clone_box()?;
        let reader = BufReader::new(stream);
        let (act_tx, act_rx) = mpsc::channel::<Act>();
        let (ui_tx, ui_rx) = mpsc::channel::<Ui>();
        let (resp_tx, resp_rx) = mpsc::channel::<Response>();

        // Broker -> UI. Losing the broker ends the control (the broker disables on its side).
        let ev_tx = ui_tx.clone();
        std::thread::spawn(move || {
            for line in reader.lines().map_while(Result::ok) {
                let Ok(mut v) = serde_json::from_str::<Value>(&line) else { continue };
                if v.get("event").is_some() {
                    if let Some(id) = v["data"]["agent"].as_str() {
                        v["agent_name"] = json!(agent_name(id));
                    }
                    let _ = ev_tx.send(Ui::Event(v));
                } else if let Ok(r) = serde_json::from_value::<Response>(v) {
                    let _ = resp_tx.send(r);
                }
            }
            eprintln!("w3bs-desktop menu bar: broker connection closed");
            std::process::exit(1);
        });

        // UI -> broker: one action at a time, off the main thread.
        let (sub_tx, sub_rx) = mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
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
            let sub = call("subscribe", json!({"native_control": true})).map(drop);
            let ok = sub.is_ok();
            let _ = sub_tx.send(sub);
            if !ok {
                return;
            }
            for act in act_rx {
                let r = match &act {
                    Act::Enable => call("enable.request", json!({})).and_then(|c| match c["code"].as_str() {
                        Some(code) => call("enable.confirm", json!({"code": code})),
                        None => Err("the broker did not hand the code to this control".into()),
                    }),
                    Act::ConfirmPending => Err("use Enable agent access; this control confirms its own requests".into()),
                    Act::Disable => call("disable", json!({"reason": "disabled from the menu bar"})),
                    Act::Stop => call("stop", json!({"reason": "stopped from the menu bar"})),
                    // The broker's own emergency-stop event: disables, and is audited as such.
                    Act::Emergency => call("session.event", json!({"event": "emergency_stop"})),
                    Act::Decide(id, ok) => call("approvals.decide", json!({"approval": id, "approve": ok})),
                    Act::Permissions => call("doctor", json!({"prompt": true})).map(|d| {
                        let checks: Vec<String> = d["checks"].as_array().into_iter().flatten().filter(|c| c["check"].as_str().is_some_and(|n| n.starts_with("macos_") && n != "macos_input_monitoring")).map(|c| format!("{}: {}", c["check"].as_str().unwrap_or("").trim_start_matches("macos_"), if c["ok"] == true { "ok" } else { "missing" })).collect();
                        json!(checks.join(", "))
                    }),
                    // The control app, signed in with a one-time link minted on this connection.
                    Act::OpenPanel => call("control.login", json!({"via": "menu bar"})).and_then(|v| {
                        crate::control_web::open_url(v["url"].as_str().unwrap_or_default()).map(|_| Value::Null).map_err(|e| format!("{e}; run `w3bs desktop control --open`"))
                    }),
                    Act::Shortcut(desc) => call("native.shortcut", json!({"description": desc})),
                    Act::Quit => {
                        let _ = call("disable", json!({"reason": "menu-bar control closed"}));
                        std::process::exit(0);
                    }
                };
                let note = match (&act, r) {
                    (Act::Permissions, Ok(v)) => Some(format!("Permissions: {}", v.as_str().unwrap_or(""))),
                    (_, Ok(_)) => None,
                    (_, Err(e)) => Some(format!("Last action failed: {e}")),
                };
                let _ = ui_tx.send(Ui::Note(note));
            }
        });
        sub_rx.recv().map_err(|_| std::io::Error::other("broker connection closed"))?.map_err(std::io::Error::other)?;

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let shortcut = {
            let tx = act_tx.clone();
            match adl_macos::hotkey::register_emergency_stop(Box::new(move || drop(tx.send(Act::Emergency)))) {
                Ok(desc) => {
                    let _ = act_tx.send(Act::Shortcut(desc.to_string()));
                    Some(desc.to_string())
                }
                Err(e) => {
                    eprintln!("w3bs-desktop menu bar: emergency-stop hot key unavailable ({e}); use the menu");
                    None
                }
            }
        };
        let target = Target::new(mtm, Ivars { item, view: RefCell::new(View { shortcut, ..View::default() }), acts: RefCell::new(vec![]), ui_rx, act_tx, log_frame_in: Cell::new(10) });
        target.rebuild();
        // SAFETY: `tick:` is implemented by `Target` and takes the timer.
        let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(0.1, &target, sel!(tick:), None, true) };
        eprintln!("w3bs-desktop menu bar: ready (emergency stop: {})", target.ivars().view.borrow().shortcut.as_deref().unwrap_or("unavailable"));
        app.run();
        Ok(())
    }
}
