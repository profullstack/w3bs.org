//! The control app: a local, installable web app for the human, served by the broker at
//! `http://127.0.0.1:7801/control` on the same loopback listener as MCP over HTTP.
//!
//! It is a native control (it can enable, approve and grant), so it is guarded like one:
//! - loopback only, and the `Host` header must be this listener's loopback address and port
//!   (DNS rebinding); a page on any other origin cannot read or drive it
//! - it is never reachable with an agent token: any request carrying `Authorization` is refused,
//!   and on Linux a browser whose process tree belongs to an agent is refused too
//! - a session starts only from a one-time URL (`/control/login?t=…`, 60 s, single use) minted
//!   over the control socket by the tray, the GNOME panel, the menu bar, the terminal control or
//!   `w3bs desktop control --open` from the person's own terminal (never an agent's process tree)
//! - the session cookie is HttpOnly, SameSite=Strict and scoped to `/control`, so it is never
//!   sent to `/mcp`; every state-changing request needs `Origin` equal to this origin, a JSON
//!   body and the session's CSRF token
//!
//! Unlike the tray and the terminal control, the app does not count as the session's native
//! control: closing the window or the browser never disables access, and enabling from the app
//! still needs a tray, panel or terminal control running (the switch that disables access when
//! it goes away). "Quit and disable" in the app disables access and signs the app out.

use crate::http_mcp::{client_pid, host_ok, Req};
use adl_broker::util::{ct_eq, now_ms, random_id, sha256_hex};
use adl_broker::{Broker, Peer};
use adl_schema::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Where the control app listens when the daemon was started without `--mcp-http`.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7801";
const LOGIN_TTL_MS: u64 = 60_000;
const SESSION_TTL_MS: u64 = 12 * 3_600_000;
const MAX_SESSIONS: usize = 16;
const MAX_STREAMS: usize = 8;
const COOKIE: &str = "w3bs_control";
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Whether the connection can carry another request.
#[derive(PartialEq, Eq)]
pub enum Next {
    KeepAlive,
    Close,
}

struct Session {
    csrf: String,
    expires_ms: u64,
}

pub struct Web {
    broker: Arc<Broker>,
    /// sha256(login token) -> expiry
    logins: Mutex<HashMap<String, u64>>,
    /// sha256(session id) -> session
    sessions: Mutex<HashMap<String, Session>>,
    /// The code of an enable request made elsewhere (CLI), as the native controls see it.
    pending: Mutex<Option<(String, u64)>>,
    shortcut: Mutex<Option<String>>,
    changes: Mutex<u64>,
    changed: Condvar,
    streams: AtomicUsize,
}

static WEB: OnceLock<Arc<Web>> = OnceLock::new();

/// Set up the control app for this broker (once per process) and follow its events.
pub fn init(broker: Arc<Broker>) -> Arc<Web> {
    WEB.get_or_init(|| {
        let w = Arc::new(Web {
            broker: broker.clone(),
            logins: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(None),
            shortcut: Mutex::new(None),
            changes: Mutex::new(0),
            changed: Condvar::new(),
            streams: AtomicUsize::new(0),
        });
        let (tx, rx) = mpsc::channel::<String>();
        // An ordinary subscriber: the app never counts toward the native-control requirement.
        broker.subscribe(tx, false);
        let wk = w.clone();
        std::thread::spawn(move || {
            for line in rx {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let d = &v["data"];
                match v["event"].as_str() {
                    Some("enable_requested") => *lock(&wk.pending) = d["code"].as_str().map(|c| (c.to_string(), d["expires_at_ms"].as_u64().unwrap_or(0))),
                    Some("state") if d["state"] != "enabling" => *lock(&wk.pending) = None,
                    _ => {}
                }
                wk.bump();
            }
        });
        w
    })
    .clone()
}

fn web() -> Option<&'static Arc<Web>> {
    WEB.get()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Web {
    fn bump(&self) {
        *lock(&self.changes) += 1;
        self.changed.notify_all();
    }

    /// A one-time login token, valid for `ttl_ms`.
    fn mint(&self, ttl_ms: u64) -> String {
        let t = random_id("login", 32);
        let now = now_ms();
        let mut l = lock(&self.logins);
        l.retain(|_, exp| *exp > now);
        l.insert(sha256_hex(t.as_bytes()), now + ttl_ms);
        t
    }

    /// Redeem a login token: single use whether or not it was still valid.
    fn redeem(&self, token: &str) -> bool {
        let exp = lock(&self.logins).remove(&sha256_hex(token.as_bytes()));
        exp.is_some_and(|e| e > now_ms())
    }

    /// Start a session; returns (session id, csrf token).
    fn open_session(&self) -> (String, String) {
        let (sid, csrf) = (random_id("sess", 32), random_id("csrf", 24));
        let now = now_ms();
        let mut s = lock(&self.sessions);
        s.retain(|_, v| v.expires_ms > now);
        while s.len() >= MAX_SESSIONS {
            let oldest = s.iter().min_by_key(|(_, v)| v.expires_ms).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => drop(s.remove(&k)),
                None => break,
            }
        }
        s.insert(sha256_hex(sid.as_bytes()), Session { csrf: csrf.clone(), expires_ms: now + SESSION_TTL_MS });
        (sid, csrf)
    }

    /// The session's key and CSRF token, if the request carries a live session cookie.
    fn session(&self, req: &Req) -> Option<(String, String)> {
        let sid = cookie(req, COOKIE)?;
        let key = sha256_hex(sid.as_bytes());
        let mut s = lock(&self.sessions);
        match s.get(&key) {
            Some(v) if v.expires_ms > now_ms() => Some((key, v.csrf.clone())),
            Some(_) => {
                s.remove(&key);
                None
            }
            None => None,
        }
    }

    fn end_session(&self, key: &str) {
        lock(&self.sessions).remove(key);
    }
}

/// Mint a one-time login URL for the control app, starting the loopback listener if the daemon
/// runs without `--mcp-http`. The caller (server.rs) has already checked that a human asked.
pub fn issue_login(via: &str) -> Result<Value> {
    let w = web().ok_or_else(|| AdlError::internal("the control app is not available in this process"))?;
    // W3BS_DESKTOP_CONTROL_ADDR (loopback only) moves it, e.g. for tests or a taken port.
    let default: SocketAddr = match std::env::var("W3BS_DESKTOP_CONTROL_ADDR") {
        Ok(a) if !a.trim().is_empty() => crate::http_mcp::parse_addr(a.trim()).map_err(AdlError::invalid)?,
        _ => DEFAULT_ADDR.parse().unwrap(),
    };
    let addr = crate::http_mcp::ensure_started(default)
        .map_err(|e| AdlError::new(ErrorCode::Blocked, format!("could not start the control app on {default} ({e}); start the daemon with --mcp-http 127.0.0.1:PORT on a free port")))?;
    let t = w.mint(LOGIN_TTL_MS);
    w.broker.audit.log("control.login", None, None, "allowed", "issued", json!({"via": via}));
    Ok(json!({"url": format!("http://{addr}/control/login?t={t}"), "expires_in_s": LOGIN_TTL_MS / 1000, "app": format!("http://{addr}/control/")}))
}

/// The native control reports the emergency-stop shortcut the desktop actually bound.
pub fn set_shortcut(desc: &str) {
    if let Some(w) = web() {
        *lock(&w.shortcut) = Some(desc.chars().take(120).collect());
        w.bump();
    }
}

/// Open a URL in the person's browser (or `$W3BS_DESKTOP_OPENER URL`, used by test rigs).
pub fn open_url(url: &str) -> std::result::Result<(), String> {
    use std::process::{Command, Stdio};
    let mut cmd = match std::env::var("W3BS_DESKTOP_OPENER") {
        Ok(o) if !o.trim().is_empty() => Command::new(o.trim()),
        _ => {
            #[cfg(target_os = "macos")]
            let c = Command::new("open");
            #[cfg(windows)]
            let c = {
                let mut c = Command::new("rundll32");
                c.arg("url.dll,FileProtocolHandler");
                c
            };
            #[cfg(not(any(target_os = "macos", windows)))]
            let c = Command::new("xdg-open");
            c
        }
    };
    let mut child = cmd.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("could not start a browser: {e}"))?;
    // Reap it without blocking the caller (xdg-open may wait for the browser on some desktops).
    std::thread::spawn(move || drop(child.wait()));
    Ok(())
}

// ------------------------------------------------------------------------------ HTTP

fn cookie(req: &Req, name: &str) -> Option<String> {
    req.headers.get("cookie")?.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn query<'a>(req: &'a Req, key: &str) -> Option<&'a str> {
    req.path.split_once('?')?.1.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn respond(w: &mut TcpStream, status: &str, ctype: &str, cache: &str, extra: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {ctype}\r\nCache-Control: {cache}\r\nContent-Security-Policy: {CSP}\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\n",
        body.len()
    );
    for (k, v) in extra {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    w.write_all(out.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

fn json_resp(w: &mut TcpStream, status: &str, extra: &[(&str, String)], v: &Value) -> std::io::Result<()> {
    respond(w, status, "application/json", "no-store", extra, v.to_string().as_bytes())
}

fn err_resp(w: &mut TcpStream, status: &str, msg: &str) -> std::io::Result<()> {
    json_resp(w, status, &[], &json!({"error": {"message": msg}}))
}

fn status_for(e: &AdlError) -> &'static str {
    match e.code {
        ErrorCode::Invalid => "400 Bad Request",
        ErrorCode::NotFound => "404 Not Found",
        ErrorCode::Denied | ErrorCode::Unauthenticated => "403 Forbidden",
        ErrorCode::Disabled | ErrorCode::Blocked | ErrorCode::Busy => "409 Conflict",
        _ => "500 Internal Server Error",
    }
}

const INDEX: &[u8] = include_bytes!("../assets/control/index.html");
const APP_JS: &[u8] = include_bytes!("../assets/control/app.js");
const APP_CSS: &[u8] = include_bytes!("../assets/control/app.css");
const SW_JS: &[u8] = include_bytes!("../assets/control/sw.js");
const MANIFEST: &[u8] = include_bytes!("../assets/control/manifest.webmanifest");
const ICON_SVG: &[u8] = include_bytes!("../assets/control/icon.svg");
const ICON_192: &[u8] = include_bytes!("../assets/control/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../assets/control/icon-512.png");

fn asset(route: &str) -> Option<(&'static str, &'static [u8])> {
    Some(match route {
        "/control/" => ("text/html; charset=utf-8", INDEX),
        "/control/app.js" => ("text/javascript; charset=utf-8", APP_JS),
        "/control/app.css" => ("text/css; charset=utf-8", APP_CSS),
        "/control/sw.js" => ("text/javascript; charset=utf-8", SW_JS),
        "/control/manifest.webmanifest" => ("application/manifest+json", MANIFEST),
        "/control/icon.svg" => ("image/svg+xml", ICON_SVG),
        "/control/icon-192.png" => ("image/png", ICON_192),
        "/control/icon-512.png" => ("image/png", ICON_512),
        _ => return None,
    })
}

/// Whether the browser making this request runs inside an agent's process tree (Linux; the
/// TCP connection is resolved to its pid). Elsewhere the pid is unknown and this is false.
fn from_agent(w: &Web, peer: SocketAddr, addr: SocketAddr) -> bool {
    client_pid(peer, addr).is_some_and(|pid| w.broker.is_agent_process(&crate::peer::lineage(pid)))
}

/// Serve one `/control` request.
pub fn serve(req: &Req, w: &mut TcpStream, addr: SocketAddr, peer: SocketAddr) -> std::io::Result<Next> {
    let host = req.headers.get("host").cloned().unwrap_or_default();
    if !host_ok(Some(&host), addr) {
        respond(w, "403 Forbidden", "text/plain; charset=utf-8", "no-store", &[], b"Host must be the loopback address of this endpoint")?;
        return Ok(Next::Close);
    }
    let Some(web) = web() else {
        err_resp(w, "503 Service Unavailable", "the control app is not running in this process")?;
        return Ok(Next::Close);
    };
    if req.headers.contains_key("authorization") {
        web.broker.audit.log("control.request", None, None, "denied", "", json!({"why": "Authorization header (agent token) on the control app"}));
        err_resp(w, "403 Forbidden", "the control app is for the person at this computer; agent tokens are not accepted here")?;
        return Ok(Next::Close);
    }
    let origin_ok = |o: &str| o.eq_ignore_ascii_case(&format!("http://{host}"));
    if let Some(o) = req.headers.get("origin")
        && !origin_ok(o)
    {
        err_resp(w, "403 Forbidden", "cross-origin requests are refused")?;
        return Ok(Next::Close);
    }
    if req.headers.get("sec-fetch-site").is_some_and(|s| s == "cross-site") {
        err_resp(w, "403 Forbidden", "cross-site requests are refused")?;
        return Ok(Next::Close);
    }
    let route = req.path.split('?').next().unwrap_or_default();
    match (req.method.as_str(), route) {
        ("GET", "/control") => respond(w, "308 Permanent Redirect", "text/plain", "no-store", &[("Location", "/control/".into())], b""),
        ("GET", "/control/login") => {
            let ok = query(req, "t").is_some_and(|t| web.redeem(t));
            if !ok {
                return respond(w, "303 See Other", "text/plain", "no-store", &[("Location", "/control/?login=expired".into())], b"").map(|_| Next::KeepAlive);
            }
            if from_agent(web, peer, addr) {
                web.broker.audit.log("control.session", None, None, "denied", "", json!({"why": "agent process tree"}));
                return err_resp(w, "403 Forbidden", "this browser was started by an agent; open the control app yourself").map(|_| Next::Close);
            }
            let (sid, _) = web.open_session();
            web.broker.audit.log("control.session", None, None, "allowed", "signed_in", Value::Null);
            respond(
                w,
                "303 See Other",
                "text/plain",
                "no-store",
                &[("Location", "/control/".into()), ("Set-Cookie", format!("{COOKIE}={sid}; Path=/control; HttpOnly; SameSite=Strict; Max-Age={}", SESSION_TTL_MS / 1000))],
                b"",
            )
        }
        ("GET", "/control/api/state") => match web.session(req) {
            Some((_, csrf)) => json_resp(w, "200 OK", &[], &state(web, &csrf, addr)),
            None => err_resp(w, "401 Unauthorized", "signed out"),
        },
        ("GET", "/control/api/audit/export") => match web.session(req) {
            Some(_) => respond(w, "200 OK", "application/json", "no-store", &[("Content-Disposition", "attachment; filename=\"w3bs-desktop-audit.json\"".into())], serde_json::to_vec_pretty(&web.broker.audit.export()).unwrap().as_slice()),
            None => err_resp(w, "401 Unauthorized", "signed out"),
        },
        ("GET", "/control/api/events") => match web.session(req) {
            Some((key, _)) => return events(web, w, &key),
            None => err_resp(w, "401 Unauthorized", "signed out"),
        },
        ("POST", "/control/api/action") => {
            let Some((key, csrf)) = web.session(req) else { return err_resp(w, "401 Unauthorized", "signed out").map(|_| Next::KeepAlive) };
            if !req.headers.get("origin").is_some_and(|o| origin_ok(o)) {
                return err_resp(w, "403 Forbidden", "Origin is required").map(|_| Next::Close);
            }
            if !req.headers.get("content-type").is_some_and(|c| c.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))) {
                return err_resp(w, "415 Unsupported Media Type", "send JSON").map(|_| Next::Close);
            }
            if !req.headers.get("x-w3bs-csrf").is_some_and(|t| ct_eq(t.as_bytes(), csrf.as_bytes())) {
                return err_resp(w, "403 Forbidden", "missing or wrong CSRF token; reload the app").map(|_| Next::Close);
            }
            if from_agent(web, peer, addr) {
                web.broker.audit.log("control.request", None, None, "denied", "", json!({"why": "agent process tree"}));
                return err_resp(w, "403 Forbidden", "this browser was started by an agent").map(|_| Next::Close);
            }
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else { return err_resp(w, "400 Bad Request", "malformed JSON").map(|_| Next::KeepAlive) };
            let pid = client_pid(peer, addr);
            let r = action(web, &body, pid, &key);
            web.bump();
            match r {
                Ok((v, signed_out)) => {
                    let extra: Vec<(&str, String)> = if signed_out { vec![("Set-Cookie", format!("{COOKIE}=; Path=/control; HttpOnly; SameSite=Strict; Max-Age=0"))] } else { vec![] };
                    json_resp(w, "200 OK", &extra, &json!({"ok": true, "result": v}))
                }
                Err(e) => json_resp(w, status_for(&e), &[], &json!({"error": e})),
            }
        }
        ("GET", r) => match asset(r) {
            Some((ctype, body)) => respond(w, "200 OK", ctype, "no-cache", &[], body),
            None => err_resp(w, "404 Not Found", "not found"),
        },
        _ => respond(w, "405 Method Not Allowed", "text/plain", "no-store", &[("Allow", "GET, POST".into())], b""),
    }
    .map(|_| Next::KeepAlive)
}

/// Server-sent events: `change` whenever broker state moves (the app then refetches).
fn events(web: &Web, w: &mut TcpStream, key: &str) -> std::io::Result<Next> {
    if web.streams.fetch_add(1, Ordering::SeqCst) >= MAX_STREAMS {
        web.streams.fetch_sub(1, Ordering::SeqCst);
        err_resp(w, "503 Service Unavailable", "too many open control app windows")?;
        return Ok(Next::Close);
    }
    struct Release<'a>(&'a AtomicUsize);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _release = Release(&web.streams);
    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nContent-Security-Policy: {CSP}\r\nX-Content-Type-Options: nosniff\r\n\r\nretry: 3000\n\n");
    w.write_all(head.as_bytes())?;
    w.flush()?;
    let mut seen = *lock(&web.changes);
    loop {
        let (g, _) = web.changed.wait_timeout_while(lock(&web.changes), Duration::from_secs(15), |c| *c == seen).unwrap_or_else(|e| e.into_inner());
        let now = *g;
        drop(g);
        let live = lock(&web.sessions).get(key).is_some_and(|s| s.expires_ms > now_ms());
        let msg = if !live {
            "event: signed_out\ndata: {}\n\n".to_string()
        } else if now != seen {
            seen = now;
            format!("event: change\ndata: {now}\n\n")
        } else {
            ": ping\n\n".into()
        };
        if w.write_all(msg.as_bytes()).and_then(|_| w.flush()).is_err() || !live {
            return Ok(Next::Close);
        }
    }
}

fn state(web: &Web, csrf: &str, addr: SocketAddr) -> Value {
    let b = &web.broker;
    let status = b.status();
    let mut tasks = b.tasks();
    tasks.reverse();
    tasks.truncate(30);
    let mut activity = b.audit.recent(60);
    activity.reverse();
    let pending = lock(&web.pending).clone().filter(|(_, exp)| *exp > now_ms() && status.state == GlobalState::Enabling);
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut shortcut = lock(&web.shortcut).clone();
    #[cfg(windows)]
    if shortcut.is_none() {
        shortcut = adl_windows::hotkey_description().map(String::from);
    }
    let mcp = crate::http_mcp::listening().is_some_and(|(_, m)| m);
    json!({
        "csrf": csrf,
        "status": status,
        "pending_enable": pending.map(|(code, exp)| json!({"code": code, "expires_at_ms": exp})),
        "approvals": b.approvals(),
        "drive": b.drive_requests(),
        "training": b.training_status(),
        "agents": b.store.list(),
        "tasks": tasks,
        "activity": activity,
        "settings": {
            "retention_days": b.audit.retention_days(),
            "shortcut": shortcut,
            "native_controls": b.native_controls(),
            "requires_native_control": b.requires_native_control(),
            "enabled_by_default": b.enabled_by_default(),
            "mcp_http": if mcp { Some(format!("http://{addr}/mcp")) } else { None },
            "app_url": format!("http://{addr}/control/"),
        },
        "now_ms": now_ms(),
    })
}

fn str_of<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| AdlError::invalid(format!("missing `{k}`")))
}

/// One human action. Returns the result and whether the session ended.
fn action(web: &Web, body: &Value, pid: Option<u32>, key: &str) -> Result<(Value, bool)> {
    let b = &web.broker;
    // A native control while the person has it open: the broker hands it the enable code, as it
    // does the tray. The agent-process-tree check already ran on the request.
    let peer = Peer { pid, agent_descendant: false, interactive: false, native_control: true };
    match str_of(body, "action")? {
        "enable" => {
            if b.enabled_by_default() {
                return to(&b.enable_unattended()?);
            }
            let ch = b.enable_request(&peer)?;
            let code = ch.code.ok_or_else(|| AdlError::internal("the broker did not hand the code to the control app"))?;
            to(&b.enable_confirm(&peer, &code)?)
        }
        "confirm_enable" => {
            let code = lock(&web.pending).take().map(|(c, _)| c).ok_or_else(|| AdlError::invalid("no enable request is waiting"))?;
            to(&b.enable_confirm(&peer, &code)?)
        }
        "disable" => to(&b.disable("disabled from the control app")),
        "stop" => to(&b.stop_all("stopped from the control app")),
        "quit" => {
            let r = b.disable("quit from the control app");
            web.end_session(key);
            b.audit.log("control.session", None, None, "allowed", "signed_out", json!({"quit": true}));
            Ok((serde_json::to_value(r).unwrap(), true))
        }
        "sign_out" => {
            web.end_session(key);
            b.audit.log("control.session", None, None, "allowed", "signed_out", Value::Null);
            Ok((json!({"signed_out": true}), true))
        }
        "approve" | "deny" => {
            let approve = str_of(body, "action")? == "approve";
            to(&b.decide_approval(&peer, str_of(body, "approval")?, approve)?)
        }
        // An agent asking to drive: allow | deny | postpone (optional postpone_ms) | always_allow.
        "drive" => {
            let decision: DriveDecision = serde_json::from_value(body.get("decision").cloned().unwrap_or(Value::Null)).map_err(|_| AdlError::invalid("decision must be allow, deny, postpone or always_allow"))?;
            to(&b.decide_drive(&peer, str_of(body, "drive")?, decision, body.get("postpone_ms").and_then(Value::as_u64))?)
        }
        "always_allow" => to(&b.set_drive_always_allow(&peer, body.get("on").and_then(Value::as_bool).unwrap_or(false))?),
        "agent_update" => {
            let a = b.store.resolve(str_of(body, "agent")?)?;
            if a.revoked {
                return Err(AdlError::invalid("a revoked agent cannot be changed; add it again"));
            }
            let apps: Vec<String> = body.get("apps").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
            if apps.iter().any(|s| s.len() > 200) || apps.len() > 64 {
                return Err(AdlError::invalid("too many or too long app ids"));
            }
            let preset = match str_of(body, "preset")? {
                "observe" => crate::Preset::Observe,
                "restricted" => crate::Preset::Restricted,
                "broad" => crate::Preset::Broad,
                "none" => crate::Preset::None,
                other => return Err(AdlError::invalid(format!("unknown preset `{other}`"))),
            };
            if preset == crate::Preset::Restricted && apps.is_empty() {
                return Err(AdlError::invalid("restricted agents need at least one app"));
            }
            // Same policy as `w3bs desktop agents update --policy <preset> --app …`, except that
            // a per-request approval requirement and the grant's expiry are kept: changing the
            // preset here must never quietly drop a safeguard.
            let mut pol = crate::preset_policy(preset, apps, a.policy.approval_required_for.iter().cloned().collect(), None);
            pol.expires_at_ms = a.policy.expires_at_ms;
            to(&b.update_agent(&peer, &a.id, pol)?)
        }
        "agent_revoke" => {
            let a = b.store.resolve(str_of(body, "agent")?)?;
            b.revoke_agent(&a.id)?;
            Ok((json!({"revoked": a.id}), false))
        }
        "task_cancel" => to(&b.cancel_task(str_of(body, "task")?, None)?),
        // Desktop Training Mode ("Start training…" in the tray opens this app at #training).
        "training_apps" => Ok((b.training_apps()?, false)),
        "training_start" => {
            let req: adl_broker::StartRequest = serde_json::from_value(json!({
                "mode": str_of(body, "mode")?,
                "goal": body.get("goal").and_then(Value::as_str),
                "applications": body.get("applications").cloned().unwrap_or(json!([])),
                "allow_agents": body.get("allow_agents").and_then(Value::as_bool).unwrap_or(false),
            }))
            .map_err(|e| AdlError::invalid(e.to_string()))?;
            Ok((b.training_start(&peer, req)?, false))
        }
        "training_pause" => Ok((b.training_pause()?, false)),
        "training_resume" => Ok((b.training_resume(&peer)?, false)),
        "training_mark" => Ok((b.training_mark()?, false)),
        "training_explain" => Ok((b.training_explain(&peer, str_of(body, "text")?)?, false)),
        "training_finish" => Ok((b.training_finish(body.get("outcome").and_then(Value::as_str))?, false)),
        "training_discard" => Ok((b.training_discard()?, false)),
        "training_forget" => {
            let minutes = body.get("minutes").and_then(Value::as_u64).filter(|m| *m > 0 && *m <= 7 * 24 * 60).ok_or_else(|| AdlError::invalid("minutes must be 1 to 10080"))?;
            Ok((b.training_forget(now_ms().saturating_sub(minutes * 60_000))?, false))
        }
        "training_handoff" => Ok((b.training_handoff()?, false)),
        // Start the W3BS Training review app; it opens itself in the browser with its own
        // one-time link and keeps running until its window's server is stopped.
        "training_open_app" => {
            let started = ["w3bs-training", "w3bs"].into_iter().find_map(|bin| {
                let mut c = std::process::Command::new(adl_broker::recorder::which(bin)?);
                if bin == "w3bs" {
                    c.arg("training");
                }
                c.arg("open").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().ok().map(|_| bin)
            });
            match started {
                Some(bin) => Ok((json!({"started": bin}), false)),
                None => Err(AdlError::new(ErrorCode::Unsupported, "W3BS Training is not installed (no w3bs-training or w3bs on PATH)")),
            }
        }
        "audit_delete" => {
            let n = b.audit.delete_all();
            b.audit.log("audit.delete", None, None, "allowed", "completed", json!({"files": n, "via": "control app"}));
            Ok((json!({"files_deleted": n}), false))
        }
        other => Err(AdlError::invalid(format!("unknown action `{other}`"))),
    }
}

fn to<T: serde::Serialize>(v: &T) -> Result<(Value, bool)> {
    Ok((serde_json::to_value(v).unwrap_or(Value::Null), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adl_broker::audit::Audit;
    use adl_broker::store::AgentStore;
    use adl_broker::Config;
    use std::io::{BufRead, BufReader, Read};

    /// One broker + control app + listener for every test in this module (the app is a
    /// per-process singleton, as in the daemon).
    fn setup() -> SocketAddr {
        static ADDR: OnceLock<SocketAddr> = OnceLock::new();
        *ADDR.get_or_init(|| {
            let cfg = Config { pace_ms: 0, require_native_control: false, drive_prompt: false, ..Config::default() };
            let broker = Arc::new(Broker::new(Arc::new(adl_broker::mock::MockDesktop::new()), AgentStore::memory(), Audit::memory(), cfg));
            init(broker);
            crate::http_mcp::spawn("127.0.0.1:0".parse().unwrap(), true).unwrap()
        })
    }

    fn get_web() -> &'static Arc<Web> {
        setup();
        web().unwrap()
    }

    /// Raw HTTP/1.1 request; returns (status, head, body).
    fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
            req.push_str(&format!("Host: {addr}\r\n"));
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).unwrap();
        let mut r = BufReader::new(s);
        let mut head = String::new();
        loop {
            let mut l = String::new();
            if r.read_line(&mut l).unwrap() == 0 || l == "\r\n" {
                break;
            }
            head.push_str(&l);
        }
        let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
        let mut body = vec![0; len];
        r.read_exact(&mut body).unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, head, String::from_utf8_lossy(&body).into())
    }

    fn header(head: &str, name: &str) -> Option<String> {
        head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
    }

    /// Log in with a fresh token; returns (cookie header value, csrf).
    fn login(addr: SocketAddr) -> (String, String) {
        let t = get_web().mint(LOGIN_TTL_MS);
        let (st, head, _) = http(addr, "GET", &format!("/control/login?t={t}"), &[], "");
        assert_eq!(st, 303);
        assert_eq!(header(&head, "location").as_deref(), Some("/control/"));
        let set = header(&head, "set-cookie").unwrap();
        for attr in ["HttpOnly", "SameSite=Strict", "Path=/control"] {
            assert!(set.contains(attr), "{set}");
        }
        let cookie = set.split(';').next().unwrap().to_string();
        let (st, _, body) = http(addr, "GET", "/control/api/state", &[("Cookie", &cookie)], "");
        assert_eq!(st, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        (cookie, v["csrf"].as_str().unwrap().to_string())
    }

    fn post(addr: SocketAddr, headers: &[(&str, &str)], body: &str) -> (u16, String) {
        let (st, _, b) = http(addr, "POST", "/control/api/action", headers, body);
        (st, b)
    }

    #[test]
    fn host_header_is_checked() {
        let addr = setup();
        for host in ["evil.example", "evil.example:7801", "127.0.0.1:1", "localhost.evil.example"] {
            let (st, _, _) = http(addr, "GET", "/control/", &[("Host", host)], "");
            assert_eq!(st, 403, "Host {host}");
        }
        let (st, head, body) = http(addr, "GET", "/control/", &[], "");
        assert_eq!(st, 200);
        assert!(body.contains("<title>"));
        let csp = header(&head, "content-security-policy").unwrap();
        assert!(csp.starts_with("default-src 'self'"), "{csp}");
        let (st, _, _) = http(addr, "GET", "/control/", &[("Host", &format!("localhost:{}", addr.port()))], "");
        assert_eq!(st, 200);
    }

    #[test]
    fn foreign_origin_is_refused() {
        let addr = setup();
        let (cookie, csrf) = login(addr);
        let (st, _, _) = http(addr, "GET", "/control/api/state", &[("Cookie", &cookie), ("Origin", "http://evil.example")], "");
        assert_eq!(st, 403);
        let base = [("Cookie", cookie.as_str()), ("Content-Type", "application/json"), ("X-W3BS-CSRF", csrf.as_str())];
        // No Origin on a state-changing request
        assert_eq!(post(addr, &base, r#"{"action":"stop"}"#).0, 403);
        // Wrong Origin
        let mut h = base.to_vec();
        h.push(("Origin", "http://attacker.test"));
        assert_eq!(post(addr, &h, r#"{"action":"stop"}"#).0, 403);
        // A cross-site fetch marker
        let own = format!("http://{addr}");
        let mut h = base.to_vec();
        h.push(("Origin", own.as_str()));
        h.push(("Sec-Fetch-Site", "cross-site"));
        assert_eq!(post(addr, &h, r#"{"action":"stop"}"#).0, 403);
        // Right Origin
        let mut h = base.to_vec();
        h.push(("Origin", own.as_str()));
        let (st, b) = post(addr, &h, r#"{"action":"stop"}"#);
        assert_eq!(st, 200, "{b}");
    }

    #[test]
    fn csrf_token_is_required() {
        let addr = setup();
        let (cookie, csrf) = login(addr);
        let own = format!("http://{addr}");
        let ok = [("Cookie", cookie.as_str()), ("Content-Type", "application/json"), ("Origin", own.as_str())];
        assert_eq!(post(addr, &ok, r#"{"action":"stop"}"#).0, 403, "no token");
        let mut h = ok.to_vec();
        h.push(("X-W3BS-CSRF", "csrf_0000"));
        assert_eq!(post(addr, &h, r#"{"action":"stop"}"#).0, 403, "wrong token");
        // Another session's token does not work either
        let (_, other) = login(addr);
        let mut h = ok.to_vec();
        h.push(("X-W3BS-CSRF", other.as_str()));
        assert_eq!(post(addr, &h, r#"{"action":"stop"}"#).0, 403, "other session's token");
        // A form post (not JSON) is refused even with the token
        let mut h = vec![("Cookie", cookie.as_str()), ("Origin", own.as_str()), ("X-W3BS-CSRF", csrf.as_str()), ("Content-Type", "application/x-www-form-urlencoded")];
        assert_eq!(post(addr, &h, "action=stop").0, 415);
        h.pop();
        h.push(("Content-Type", "application/json"));
        assert_eq!(post(addr, &h, r#"{"action":"stop"}"#).0, 200);
        // No cookie at all
        let h = [("Content-Type", "application/json"), ("Origin", own.as_str()), ("X-W3BS-CSRF", csrf.as_str())];
        assert_eq!(post(addr, &h, r#"{"action":"stop"}"#).0, 401);
    }

    #[test]
    fn agent_tokens_never_reach_the_control_app() {
        let addr = setup();
        let w = get_web();
        let (_, token) = w.broker.store.add("ctl-agent", Policy::default()).unwrap();
        let bearer = format!("Bearer {token}");
        let (cookie, csrf) = login(addr);
        let own = format!("http://{addr}");
        for (m, p) in [("GET", "/control/"), ("GET", "/control/app.js"), ("GET", "/control/api/state"), ("GET", "/control/api/events"), ("GET", "/control/api/audit/export"), ("GET", "/control/login?t=x"), ("POST", "/control/api/action")] {
            let (st, _, _) = http(addr, m, p, &[("Authorization", &bearer)], "");
            assert!(st == 401 || st == 403, "{m} {p}: {st}");
            // Even with a valid session alongside it
            let (st, _, _) = http(addr, m, p, &[("Authorization", &bearer), ("Cookie", &cookie), ("Origin", &own), ("X-W3BS-CSRF", &csrf), ("Content-Type", "application/json")], r#"{"action":"stop"}"#);
            assert!(st == 401 || st == 403, "{m} {p} with session: {st}");
        }
        // And the control cookie is not a credential on /mcp
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let (st, _, _) = http(addr, "POST", "/mcp", &[("Cookie", &cookie), ("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream")], init);
        assert_eq!(st, 401);
    }

    #[test]
    fn login_token_is_single_use() {
        let addr = setup();
        let t = get_web().mint(LOGIN_TTL_MS);
        let (st, head, _) = http(addr, "GET", &format!("/control/login?t={t}"), &[], "");
        assert_eq!(st, 303);
        assert!(header(&head, "set-cookie").is_some());
        let (st, head, _) = http(addr, "GET", &format!("/control/login?t={t}"), &[], "");
        assert_eq!(st, 303);
        assert_eq!(header(&head, "location").as_deref(), Some("/control/?login=expired"));
        assert!(header(&head, "set-cookie").is_none(), "a reused token must not sign in");
        let (st, head, _) = http(addr, "GET", "/control/login?t=login_deadbeef", &[], "");
        assert_eq!(st, 303);
        assert!(header(&head, "set-cookie").is_none());
    }

    #[test]
    fn login_token_expires() {
        let addr = setup();
        let t = get_web().mint(40);
        std::thread::sleep(Duration::from_millis(120));
        let (_, head, _) = http(addr, "GET", &format!("/control/login?t={t}"), &[], "");
        assert!(header(&head, "set-cookie").is_none(), "expired token signed in");
        assert_eq!(header(&head, "location").as_deref(), Some("/control/?login=expired"));
    }

    #[test]
    fn signed_out_requests_get_401() {
        let addr = setup();
        let (st, _, _) = http(addr, "GET", "/control/api/state", &[], "");
        assert_eq!(st, 401);
        let (st, _, _) = http(addr, "GET", "/control/api/state", &[("Cookie", "w3bs_control=sess_forged")], "");
        assert_eq!(st, 401);
        // sign out ends the session
        let (cookie, csrf) = login(addr);
        let own = format!("http://{addr}");
        let h = [("Cookie", cookie.as_str()), ("Content-Type", "application/json"), ("Origin", own.as_str()), ("X-W3BS-CSRF", csrf.as_str())];
        assert_eq!(post(addr, &h, r#"{"action":"sign_out"}"#).0, 200);
        let (st, _, _) = http(addr, "GET", "/control/api/state", &[("Cookie", &cookie)], "");
        assert_eq!(st, 401);
    }

    #[test]
    fn mcp_routes_stay_separate() {
        let addr = setup();
        // /mcp still refuses browsers and needs its own bearer
        let (st, _, _) = http(addr, "POST", "/mcp", &[("Origin", &format!("http://{addr}"))], "{}");
        assert_eq!(st, 403);
        let (st, _, _) = http(addr, "GET", "/elsewhere", &[], "");
        assert_eq!(st, 404);
    }
}
