//! The local broker daemon: two Unix sockets, one JSON request per line.
//!
//! `agent.sock` serves authenticated agents and has no human-only methods. `control.sock` serves
//! the native control and the human CLI; requests that raise access are refused when they come
//! from an agent's process tree. Both sockets are 0600 inside a 0700 runtime directory, and
//! every peer must be the same uid.

use crate::paths;
use crate::peer;
use adl_broker::{Broker, Peer, Principal, SessionEvent};
use adl_schema::rpc::{Request, Response, AGENT_METHODS, CONTROL_METHODS};
use adl_schema::*;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const MAX_LINE: u64 = 256 * 1024;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("{} is already served by a running daemon", path.display())));
        }
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

pub fn serve(broker: Arc<Broker>) -> std::io::Result<()> {
    let dir = paths::runtime_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let agent = bind(&paths::agent_socket())?;
    let control = bind(&paths::control_socket())?;
    // SAFETY: installing a handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
    }
    eprintln!("w3bs-desktop: broker ready (adapter {}), state Disabled", broker.adapter_name());
    eprintln!("  agents:  {}", paths::agent_socket().display());
    eprintln!("  control: {}", paths::control_socket().display());

    for (listener, kind) in [(agent, Kind::Agent), (control, Kind::Control)] {
        let b = broker.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let b = b.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(b, conn, kind) {
                        if e.kind() != std::io::ErrorKind::BrokenPipe {
                            eprintln!("w3bs-desktop: connection error: {e}");
                        }
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
    let _ = std::fs::remove_file(paths::agent_socket());
    let _ = std::fs::remove_file(paths::control_socket());
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Agent,
    Control,
}

struct Conn {
    kind: Kind,
    cred: Option<peer::Cred>,
    principal: Option<Principal>,
    native_control: bool,
    writer: Arc<Mutex<UnixStream>>,
}

fn write_line(w: &Mutex<UnixStream>, line: &str) -> std::io::Result<()> {
    let mut s = w.lock().unwrap_or_else(|e| e.into_inner());
    s.write_all(line.as_bytes())?;
    s.write_all(b"\n")
}

fn handle(broker: Arc<Broker>, stream: UnixStream, kind: Kind) -> std::io::Result<()> {
    let cred = peer::peer_cred(&stream);
    if let Some(c) = cred {
        if c.uid != peer::my_uid() {
            return Ok(()); // other users never get an answer
        }
    }
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
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
        let mcp = p.get("client").and_then(Value::as_str) == Some("mcp");
        let root = c.cred.and_then(|cr| if mcp { peer::parent_of(cr.pid) } else { Some(cr.pid) });
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
        "doctor" => Ok(doctor(b)),
        "enable.request" => to(b.enable_request(&peer)?),
        "enable.confirm" => to(b.enable_confirm(&peer, &str_param(p, "code")?)?),
        "disable" => to(b.disable(&reason())),
        "stop" => to(b.stop_all(&reason())),
        "takeover" => to(b.takeover()),
        "agents.add" => {
            let policy: Policy = p.get("policy").map(|v| params(v)).transpose()?.unwrap_or_default();
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

fn doctor(b: &Broker) -> Value {
    let caps = b.capabilities();
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let mut checks = vec![];
    let mut check = |name: &str, ok: bool, detail: String| checks.push(json!({"check": name, "ok": ok, "detail": detail}));
    check("runtime_dir_private", std::fs::metadata(paths::runtime_dir()).map(|m| m.permissions().mode() & 0o077 == 0).unwrap_or(false), paths::runtime_dir().display().to_string());
    check("session", !session.is_empty(), format!("XDG_SESSION_TYPE={session:?} XDG_CURRENT_DESKTOP={desktop:?}"));
    let input_ok = caps.iter().any(|c| c.capability == Capability::Pointer && c.availability == Availability::Available);
    check("pointer_backend", input_ok, b.adapter_name());
    check("native_control", true, "the tray, panel extension or `w3bs desktop control` must be running before enable".into());
    json!({"status": b.status(), "capabilities": caps, "checks": checks, "paths": {
        "agent_socket": paths::agent_socket(), "control_socket": paths::control_socket(), "config": paths::config_dir(), "state": paths::state_dir()}})
}
