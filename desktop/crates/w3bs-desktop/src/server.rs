//! The local broker daemon: two local endpoints, one JSON request per line.
//!
//! The agent endpoint serves authenticated agents and has no human-only methods. The control
//! endpoint serves the native control and the human CLI; requests that raise access are refused
//! when they come from an agent's process tree. Endpoints are private to the user (0600 sockets
//! in a 0700 directory on Linux/macOS, a user-only named pipe on Windows), and every peer must be
//! the same user.

use crate::paths;
use crate::peer;
use crate::transport::{Duplex, Listener, PeerInfo};
use adl_broker::{Broker, Peer, Principal, SessionEvent};
use adl_schema::rpc::{Request, Response, AGENT_METHODS, CONTROL_METHODS};
use adl_schema::*;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const MAX_LINE: u64 = 256 * 1024;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
fn install_shutdown_handlers() {
    // SAFETY: installing a handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
    }
}

/// Set once the supervisor has disabled access on the way out.
#[cfg_attr(not(windows), allow(dead_code))]
static SHUT_DOWN: AtomicBool = AtomicBool::new(false);

/// Console control events: Ctrl+C, Ctrl+Break, console close, logoff and system shutdown all
/// disable access and exit. For close/logoff/shutdown Windows ends the process once the handler
/// returns, so the handler waits (bounded) until the supervisor has disabled.
#[cfg(windows)]
fn install_shutdown_handlers() {
    unsafe extern "system" fn on_ctrl(_kind: u32) -> windows::core::BOOL {
        request_shutdown();
        let t = std::time::Instant::now();
        while !SHUT_DOWN.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        windows::core::BOOL(1)
    }
    // SAFETY: the handler only touches atomics and sleeps.
    if let Err(e) = unsafe { windows::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), true) } {
        eprintln!("w3bs-desktop: console control handler unavailable ({e}); Ctrl+C will not disable cleanly");
    }
}

/// Ask the supervisor loop to disable and exit (used by platform shutdown hooks).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

pub fn serve(broker: Arc<Broker>) -> std::io::Result<()> {
    let (agent_ep, control_ep) = (paths::agent_socket(), paths::control_socket());
    let agent = Listener::bind(&agent_ep)?;
    let control = Listener::bind(&control_ep)?;
    install_shutdown_handlers();
    eprintln!("w3bs-desktop: broker ready (adapter {}), state Disabled", broker.adapter_name());
    eprintln!("  agents:  {agent_ep}");
    eprintln!("  control: {control_ep}");

    for (listener, kind) in [(agent, Kind::Agent), (control, Kind::Control)] {
        let b = broker.clone();
        std::thread::spawn(move || {
            loop {
                let Ok((conn, peer)) = listener.accept() else { continue };
                let b = b.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(b, conn, peer, kind)
                        && e.kind() != std::io::ErrorKind::BrokenPipe
                    {
                        eprintln!("w3bs-desktop: connection error: {e}");
                    }
                });
            }
        });
    }

    // Supervisor: session events (lock, suspend, takeover) and shutdown.
    while !SHUTDOWN.load(Ordering::SeqCst) {
        broker.pump_session_events();
        std::thread::sleep(Duration::from_millis(50));
    }
    let report = broker.disable("broker shutting down");
    eprintln!("w3bs-desktop: disabled on shutdown ({} tasks cancelled)", report.tasks_cancelled);
    Listener::remove(&agent_ep);
    Listener::remove(&control_ep);
    SHUT_DOWN.store(true, Ordering::SeqCst);
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Agent,
    Control,
}

struct Conn {
    kind: Kind,
    cred: Option<PeerInfo>,
    principal: Option<Principal>,
    native_control: bool,
    writer: Arc<Mutex<Box<dyn Duplex>>>,
}

fn write_line(w: &Mutex<Box<dyn Duplex>>, line: &str) -> std::io::Result<()> {
    let mut s = w.lock().unwrap_or_else(|e| e.into_inner());
    s.write_all(line.as_bytes())?;
    s.write_all(b"\n")?;
    s.flush()
}

fn handle(broker: Arc<Broker>, stream: Box<dyn Duplex>, cred: Option<PeerInfo>, kind: Kind) -> std::io::Result<()> {
    if cred.is_some_and(|c| !c.same_user) {
        return Ok(()); // other users never get an answer
    }
    let writer = Arc::new(Mutex::new(stream.try_clone_box()?));
    let mut conn = Conn { kind, cred, principal: None, native_control: false, writer: writer.clone() };
    let mut reader = BufReader::new(stream);
    let result = (|| -> std::io::Result<()> {
        loop {
            let mut line = String::new();
            let n = (&mut reader).take(MAX_LINE).read_line(&mut line)?;
            if n == 0 {
                return Ok(());
            }
            if !line.ends_with('\n') && n as u64 >= MAX_LINE {
                write_line(&writer, &serde_json::to_string(&Response::err(String::new(), AdlError::invalid("request too large"))).unwrap())?;
                return Ok(());
            }
            let resp = match serde_json::from_str::<Request>(line.trim()) {
                Ok(req) => {
                    let id = req.id.clone();
                    match dispatch(&broker, &mut conn, req) {
                        Ok(v) => Response::ok(id, v),
                        Err(e) => Response::err(id, e),
                    }
                }
                Err(e) => Response::err(String::new(), AdlError::invalid(format!("malformed request: {e}"))),
            };
            write_line(&writer, &serde_json::to_string(&resp).unwrap())?;
        }
    })();
    if conn.native_control {
        broker.detach_native_control();
    }
    if let Some(p) = &conn.principal {
        broker.end_session(p);
    }
    result
}


fn params<T: DeserializeOwned>(v: &Value) -> Result<T> {
    serde_json::from_value(if v.is_null() { json!({}) } else { v.clone() }).map_err(|e| AdlError::invalid(format!("invalid params: {e}")))
}

fn str_param(v: &Value, key: &str) -> Result<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| AdlError::invalid(format!("missing string param `{key}`")))
}

fn to<T: serde::Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v).unwrap())
}

fn dispatch(b: &Arc<Broker>, c: &mut Conn, req: Request) -> Result<Value> {
    if req.v != SCHEMA_VERSION {
        return Err(AdlError::invalid(format!("unsupported schema version {}; this broker speaks {SCHEMA_VERSION}", req.v)));
    }
    match c.kind {
        Kind::Agent => agent(b, c, &req),
        Kind::Control => control(b, c, &req),
    }
}

fn agent(b: &Arc<Broker>, c: &mut Conn, req: &Request) -> Result<Value> {
    if !AGENT_METHODS.contains(&req.method.as_str()) {
        return Err(AdlError::denied(format!("`{}` is not available to agents", req.method)));
    }
    let p = &req.params;
    if req.method == "hello" {
        let token = str_param(p, "token")?;
        // For an MCP client, the app that spawned the server (Claude Code, OpenCode, Codex)
        // becomes an agent root, so the shell commands that app runs cannot raise access on
        // control.sock. One-shot CLI clients only mark themselves, so a human using an agent
        // token from their own terminal keeps their own shell.
        let kind = p.get("client").and_then(Value::as_str).unwrap_or("cli");
        // The in-process HTTP MCP bridge connects as the broker itself; it passes the pid of the
        // HTTP client it resolved from the TCP connection (the agent app itself). Only trusted
        // when the peer really is this process; never mark the broker's own parent.
        let in_process = c.cred.is_some_and(|cr| cr.pid == std::process::id());
        let root = match kind {
            "http" if in_process => p.get("client_pid").and_then(Value::as_u64).map(|v| v as u32),
            "http" => return Err(AdlError::denied("the http client kind is reserved for the broker's own MCP endpoint")),
            "mcp" => c.cred.and_then(|cr| peer::parent_of(cr.pid)),
            _ => c.cred.map(|cr| cr.pid),
        };
        let pr = b.authenticate(&token, root)?;
        let agent = b.store.get(&pr.agent).unwrap();
        c.principal = Some(pr.clone());
        return Ok(json!({"agent": agent.id, "name": agent.name, "session": pr.session, "policy": agent.policy, "schema": SCHEMA_VERSION}));
    }
    let pr = c.principal.clone().ok_or_else(|| AdlError::new(ErrorCode::Unauthenticated, "send hello with your agent token first"))?;
    match req.method.as_str() {
        "status" => {
            let s = b.status();
            let mine = |l: &LeaseInfo| l.agent == pr.agent;
            Ok(json!({"schema": SCHEMA_VERSION, "state": s.state, "adapter": s.adapter, "holds_lease": s.lease.as_ref().is_some_and(mine), "lease_busy": s.lease.is_some(), "tasks": b.tasks_for(&pr)}))
        }
        "capabilities" => to(b.capabilities()),
        "layout" => to(b.layout(&pr)?),
        "windows" => to(b.windows(&pr)?),
        "inspect" => to(b.inspect(&pr, &str_param(p, "window")?)?),
        "screenshot" => to(b.screenshot(&pr, &str_param(p, "display")?)?),
        "pointer.position" => to(b.pointer_position(&pr)?),
        "task.begin" => to(b.task_begin(&pr, p.get("description").and_then(Value::as_str).unwrap_or(""))?),
        "task.end" => {
            let outcome = p.get("outcome").map(|o| serde_json::from_value(o.clone())).transpose().map_err(|e| AdlError::invalid(e.to_string()))?.unwrap_or(TaskState::Completed);
            to(b.task_end(&pr, &str_param(p, "task")?, outcome)?)
        }
        "task.cancel" => to(b.cancel_task(&str_param(p, "task")?, Some(&pr))?),
        "task.list" => to(b.tasks_for(&pr)),
        "task.inspect" => {
            let t = b.task(&str_param(p, "task")?)?;
            if t.agent != pr.agent {
                return Err(AdlError::not_found("no such task"));
            }
            to(t)
        }
        "lease.acquire" => to(b.lease_acquire(&pr, &str_param(p, "task")?, p.get("ttl_ms").and_then(Value::as_u64))?),
        "lease.renew" => to(b.lease_renew(&pr, p.get("ttl_ms").and_then(Value::as_u64))?),
        "lease.release" => to(b.lease_release(&pr)?),
        "input" => to(b.input(&pr, &params::<InputBatch>(p)?)?),
        "approval.status" => to(b.approval_status(&pr, &str_param(p, "approval")?)?),
        m => Err(AdlError::denied(format!("`{m}` is not available to agents"))),
    }
}

fn control_peer(b: &Broker, c: &Conn) -> Peer {
    match c.cred {
        Some(cr) => Peer { pid: Some(cr.pid), agent_descendant: b.is_agent_process(&peer::lineage(cr.pid)), interactive: peer::has_tty(cr.pid), native_control: c.native_control },
        // Without peer credentials we cannot tell who is asking, so assume the worst.
        None => Peer { pid: None, agent_descendant: true, interactive: false, native_control: false },
    }
}

fn control(b: &Arc<Broker>, c: &mut Conn, req: &Request) -> Result<Value> {
    if !CONTROL_METHODS.contains(&req.method.as_str()) {
        return Err(AdlError::invalid(format!("unknown method `{}`", req.method)));
    }
    let p = &req.params;
    let peer = control_peer(b, c);
    let reason = || p.get("reason").and_then(Value::as_str).unwrap_or("requested from the control").to_string();
    match req.method.as_str() {
        "status" => to(b.status()),
        "capabilities" => to(b.capabilities()),
        "doctor" => {
            // Showing the OS permission prompts is a human action, like enabling.
            let prompt = p.get("prompt").and_then(Value::as_bool).unwrap_or(false);
            if prompt && peer.agent_descendant {
                return Err(AdlError::denied("doctor --prompt must come from the human, not from an agent's process tree"));
            }
            Ok(doctor(b, prompt))
        }
        // A non-desktop broker enables straight away and answers with its status, not a challenge.
        "enable.request" if b.enabled_by_default() => {
            if peer.agent_descendant {
                return Err(AdlError::denied("enable.request must come from the human, not from an agent's process tree"));
            }
            to(b.enable_unattended()?)
        }
        "enable.request" => to(b.enable_request(&peer)?),
        "enable.confirm" => to(b.enable_confirm(&peer, &str_param(p, "code")?)?),
        "disable" => to(b.disable(&reason())),
        "stop" => to(b.stop_all(&reason())),
        "takeover" => to(b.takeover()),
        "agents.add" => {
            let policy: Policy = p.get("policy").map(params).transpose()?.unwrap_or_default();
            let (info, token) = b.add_agent(&peer, &str_param(p, "name")?, policy)?;
            Ok(json!({"agent": info, "token": token}))
        }
        "agents.list" => to(b.store.list()),
        "agents.update" => {
            let a = b.store.resolve(&str_param(p, "agent")?)?;
            to(b.update_agent(&peer, &a.id, params(p.get("policy").unwrap_or(&Value::Null))?)?)
        }
        "agents.revoke" => {
            let a = b.store.resolve(&str_param(p, "agent")?)?;
            b.revoke_agent(&a.id)?;
            Ok(json!({"revoked": a.id}))
        }
        "agents.remove" => {
            let a = b.store.resolve(&str_param(p, "agent")?)?;
            b.remove_agent(&a.id)?;
            Ok(json!({"removed": a.id}))
        }
        "tasks.list" => to(b.tasks()),
        "tasks.inspect" => to(b.task(&str_param(p, "task")?)?),
        "tasks.cancel" => to(b.cancel_task(&str_param(p, "task")?, None)?),
        "approvals.list" => to(b.approvals()),
        "approvals.decide" => to(b.decide_approval(&peer, &str_param(p, "approval")?, p.get("approve").and_then(Value::as_bool).unwrap_or(false))?),
        "audit.export" => to(b.audit.export()),
        "audit.delete" => Ok(json!({"files_deleted": b.audit.delete_all()})),
        "audit.recent" => to(b.audit.recent(p.get("limit").and_then(Value::as_u64).unwrap_or(20).min(500) as usize)),
        // A one-time sign-in link for the control app. The app can enable, approve and grant, so
        // only the human may ask: the native control itself, or an interactive terminal outside
        // any agent's process tree (the same bar as confirming an enable).
        "control.login" => {
            let via = p.get("via").and_then(Value::as_str).unwrap_or("control").chars().take(16).collect::<String>();
            if peer.agent_descendant || !(peer.native_control || peer.interactive) {
                b.audit.log("control.login", None, None, "denied", "", json!({"via": via, "pid": peer.pid, "why": if peer.agent_descendant { "agent process tree" } else { "not the native control or an interactive terminal" }}));
                return Err(AdlError::denied("open the control app from the tray, panel or menu bar, or run `w3bs desktop control --open` in your own terminal"));
            }
            crate::control_web::issue_login(&via)
        }
        // The native control reports the emergency-stop shortcut the desktop bound, for the app.
        "native.shortcut" => {
            if !c.native_control {
                return Err(AdlError::denied("only the subscribed native control reports its shortcut"));
            }
            crate::control_web::set_shortcut(&str_param(p, "description")?);
            Ok(json!({"ok": true}))
        }
        "session.event" => {
            // Only events that reduce access are accepted from the control socket.
            let e = match str_param(p, "event")?.as_str() {
                "locked" => SessionEvent::Locked,
                "suspending" => SessionEvent::Suspending,
                "logging_out" => SessionEvent::LoggingOut,
                "emergency_stop" => SessionEvent::EmergencyStop,
                "physical_input" => SessionEvent::PhysicalInput,
                other => return Err(AdlError::invalid(format!("unsupported session event `{other}`"))),
            };
            b.handle_session_event(e);
            to(b.status())
        }
        "subscribe" => {
            let native = p.get("native_control").and_then(Value::as_bool).unwrap_or(false);
            if native && c.native_control {
                return Err(AdlError::invalid("already subscribed"));
            }
            let (tx, rx) = mpsc::channel::<String>();
            let w = c.writer.clone();
            std::thread::spawn(move || {
                for line in rx {
                    if write_line(&w, &line).is_err() {
                        break;
                    }
                }
            });
            b.subscribe(tx, native);
            c.native_control |= native;
            Ok(json!({"subscribed": true, "native_control": native}))
        }
        m => Err(AdlError::invalid(format!("unknown method `{m}`"))),
    }
}

#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
fn doctor(b: &Broker, prompt: bool) -> Value {
    let caps = b.capabilities();
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let mut checks = vec![];
    let mut check = |name: &str, ok: bool, detail: String| checks.push(json!({"check": name, "ok": ok, "detail": detail}));
    check("endpoints_private", endpoints_private(), paths::control_socket().to_string());
    #[cfg(not(any(target_os = "macos", windows)))]
    check("session", !session.is_empty(), format!("XDG_SESSION_TYPE={session:?} XDG_CURRENT_DESKTOP={desktop:?}"));
    #[cfg(target_os = "macos")]
    {
        let _ = (&session, &desktop);
        let (locked, on_console) = adl_macos::adapter::session_state();
        check("session", on_console && !locked, format!("macOS window server session: on console {on_console}, locked {locked}"));
        // TCC permissions of this (the broker's) process. `prompt` shows the system prompts.
        for (name, ok, detail) in crate::platform::permission_checks(prompt) {
            check(name, ok, detail);
        }
    }
    #[cfg(windows)]
    {
        let _ = (&session, &desktop);
        // Session 0 is where services run; it has no interactive desktop.
        let id = adl_windows::win::current_session();
        check("session", id.is_some_and(|s| s != 0), format!("session {id:?} SESSIONNAME={:?}", std::env::var("SESSIONNAME").unwrap_or_default()));
        let hk = adl_windows::hotkey_description();
        check("emergency_hotkey", hk.is_some(), hk.map(|h| format!("{h} disables access")).unwrap_or_else(|| "no emergency chord registered (not the native adapter, or every chord is taken); use the tray or `w3bs desktop disable`".into()));
    }
    let input_ok = caps.iter().any(|c| c.capability == Capability::Pointer && c.availability == Availability::Available);
    check("pointer_backend", input_ok, b.adapter_name());
    #[cfg(target_os = "linux")]
    if adl_linux::gnome::detect() {
        match adl_linux::gnome::extension_present() {
            Ok(owner) => check("gnome_shell_extension", true, format!("{} is loaded in gnome-shell ({owner})", adl_linux::gnome::EXTENSION_UUID)),
            Err(e) => check("gnome_shell_extension", false, format!("{}; {}", e.message, if e.message.contains("setup gnome-extension") { "" } else { adl_linux::gnome::INSTALL_HINT })),
        }
    }
    if b.enabled_by_default() {
        check("native_control", true, "not required: started with --enabled-by-default (non-desktop instance)".into());
    } else {
        check("native_control", true, "the tray, panel extension or `w3bs desktop control` must be running before enable".into());
    }
    json!({"status": b.status(), "capabilities": caps, "checks": checks, "paths": {
        "agent_socket": paths::agent_socket().to_string(), "control_socket": paths::control_socket().to_string(), "config": paths::config_dir(), "state": paths::state_dir()}})
}

#[cfg(unix)]
fn endpoints_private() -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(paths::runtime_dir()).map(|m| m.permissions().mode() & 0o077 == 0).unwrap_or(false)
}

#[cfg(windows)]
fn endpoints_private() -> bool {
    // The broker could not have started otherwise: both pipes are created (first instance, no
    // squatter) with an owner of the current user and a protected DACL granting only that user.
    // Token and state files live under %LOCALAPPDATA%, whose inherited ACL grants the user,
    // SYSTEM and Administrators (as root can read 0600 files on Unix).
    true
}
