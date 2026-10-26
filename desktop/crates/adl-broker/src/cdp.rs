//! A small synchronous Chrome DevTools Protocol client for the browser channel.
//!
//! The broker talks to one Chromium-family browser (TronBrowser first) over its loopback
//! DevTools port, the same protocol chrome-devtools-mcp drives. Pages are reached through
//! flattened target sessions on the one browser connection. Events are read and dropped: every
//! call waits only for its own response.
//!
//! Finding the browser, in order:
//! 1. `W3BS_DESKTOP_BROWSER_CDP` (or `daemon --browser-cdp`): `http://127.0.0.1:9222` or a
//!    `ws://…/devtools/browser/…` URL.
//! 2. TronBrowser's managed session (`tron browser launch`): `~/.tronbrowser/automation/session.json`.
//! 3. A `DevToolsActivePort` file in a TronBrowser, Chromium or Chrome profile.
//! 4. `127.0.0.1:9222`.

use adl_schema::{AdlError, ErrorCode};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// One place DevTools might be listening.
#[derive(Clone, Debug)]
struct Candidate {
    source: String,
    /// `host:port`
    addr: String,
    /// The browser WebSocket path, when known without asking `/json/version`.
    ws_path: Option<String>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))
}

/// Profile directories whose `DevToolsActivePort` we look at.
fn profile_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(h) = home() {
        // TronBrowser: its main profile and every `tron browser launch --profile NAME` profile.
        v.push(h.join(".tronbrowser"));
        if let Ok(rd) = std::fs::read_dir(&h) {
            let mut extra: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".tronbrowser-"))).collect();
            extra.sort();
            v.extend(extra);
        }
        v.push(h.join("snap/chromium/common/chromium"));
        v.push(h.join("Library/Application Support/Google/Chrome"));
        v.push(h.join("Library/Application Support/Chromium"));
    }
    if let Some(c) = config_home() {
        v.push(c.join("chromium"));
        v.push(c.join("google-chrome"));
    }
    v
}

fn candidates(explicit: Option<&str>) -> Vec<Candidate> {
    let mut v = vec![];
    if let Some(e) = explicit.map(str::trim).filter(|e| !e.is_empty()) {
        let rest = e.trim_start_matches("http://").trim_start_matches("ws://");
        let (addr, path) = match rest.split_once('/') {
            Some((a, p)) => (a.to_string(), Some(format!("/{p}"))),
            None => (rest.to_string(), None),
        };
        let ws_path = path.filter(|p| p.starts_with("/devtools/"));
        v.push(Candidate { source: "W3BS_DESKTOP_BROWSER_CDP".into(), addr, ws_path });
        return v;
    }
    if let Some(h) = home() {
        let desc = h.join(".tronbrowser/automation/session.json");
        if let Some(d) = std::fs::read_to_string(&desc).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok())
            && let Some(port) = d["port"].as_u64()
        {
            let host = d["host"].as_str().unwrap_or("127.0.0.1");
            let ws_path = d["webSocketDebuggerUrl"].as_str().and_then(|u| u.find("/devtools/").map(|i| u[i..].to_string()));
            v.push(Candidate { source: format!("TronBrowser managed session ({})", desc.display()), addr: format!("{host}:{port}"), ws_path });
        }
    }
    for dir in profile_dirs() {
        let f = dir.join("DevToolsActivePort");
        if let Ok(s) = std::fs::read_to_string(&f) {
            let mut lines = s.lines();
            if let Some(port) = lines.next().and_then(|p| p.trim().parse::<u16>().ok()) {
                let ws_path = lines.next().map(|p| p.trim().to_string()).filter(|p| p.starts_with("/devtools/"));
                v.push(Candidate { source: f.display().to_string(), addr: format!("127.0.0.1:{port}"), ws_path });
            }
        }
    }
    v.push(Candidate { source: "default port".into(), addr: "127.0.0.1:9222".into(), ws_path: None });
    v
}

fn tcp(addr: &str) -> std::io::Result<TcpStream> {
    let sa: SocketAddr = addr.to_socket_addrs()?.next().ok_or_else(|| std::io::Error::other("no address"))?;
    let s = TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT)?;
    s.set_nodelay(true).ok();
    Ok(s)
}

/// `GET path` on the DevTools HTTP endpoint.
fn http_get_json(addr: &str, path: &str) -> std::io::Result<Value> {
    let mut s = tcp(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")?;
    let mut buf = vec![];
    let mut chunk = [0u8; 8192];
    // Read until the declared body length arrives or the server closes.
    loop {
        let n = match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if buf.is_empty() => return Err(e),
            Err(_) => break,
        };
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
            let len = head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
            if len.is_some_and(|l| buf.len() >= i + 4 + l) {
                break;
            }
        }
    }
    let i = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| std::io::Error::other("not an HTTP response"))?;
    serde_json::from_slice(&buf[i + 4..]).map_err(std::io::Error::other)
}

pub fn unavailable(tried: Vec<String>) -> AdlError {
    AdlError::new(
        ErrorCode::Blocked,
        "DevTools cannot control the browser: no Chromium/TronBrowser with a DevTools port is reachable. Start the agent browser with `w3bs desktop browser launch` (or set W3BS_DESKTOP_BROWSER_CDP); the screenshot tools are only a fallback",
    )
    .with_details(json!({"browser_unavailable": true, "tried": tried, "fix": "w3bs desktop browser launch"}))
}

fn transport(e: impl std::fmt::Display) -> AdlError {
    AdlError::new(ErrorCode::Blocked, format!("DevTools connection lost: {e}")).with_details(json!({"browser_unavailable": true, "cdp": "transport"}))
}

pub fn is_transport(e: &AdlError) -> bool {
    e.details.get("cdp").and_then(Value::as_str) == Some("transport")
}

pub struct Cdp {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    pub source: String,
    pub addr: String,
    pub product: String,
    /// target id -> flattened session id
    sessions: HashMap<String, String>,
}

impl Cdp {
    /// Find a browser and open its browser-level DevTools WebSocket.
    pub fn connect(explicit: Option<&str>) -> Result<Self, AdlError> {
        let mut tried = vec![];
        for c in candidates(explicit) {
            match Self::open(&c) {
                Ok(cdp) => return Ok(cdp),
                Err(e) => tried.push(format!("{} ({}): {e}", c.addr, c.source)),
            }
        }
        Err(unavailable(tried))
    }

    fn open(c: &Candidate) -> Result<Self, String> {
        let version = http_get_json(&c.addr, "/json/version").ok();
        let path = match (&c.ws_path, &version) {
            (Some(p), _) => p.clone(),
            (None, Some(v)) => v["webSocketDebuggerUrl"].as_str().and_then(|u| u.find("/devtools/").map(|i| u[i..].to_string())).ok_or("no webSocketDebuggerUrl")?,
            (None, None) => return Err("nothing listening".into()),
        };
        let stream = tcp(&c.addr).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(CALL_TIMEOUT)).ok();
        let url = format!("ws://{}{}", c.addr, path);
        let cfg = WebSocketConfig::default().max_message_size(Some(256 << 20)).max_frame_size(Some(256 << 20));
        let (ws, _) = tungstenite::client::client_with_config(url.as_str(), stream, Some(cfg)).map_err(|e| e.to_string())?;
        let product = version.as_ref().and_then(|v| v["Browser"].as_str()).unwrap_or("").to_string();
        let mut cdp = Cdp { ws, next_id: 0, source: c.source.clone(), addr: c.addr.clone(), product, sessions: HashMap::new() };
        if cdp.product.is_empty()
            && let Ok(v) = cdp.call(None, "Browser.getVersion", json!({}))
        {
            cdp.product = v["product"].as_str().unwrap_or("").to_string();
        }
        Ok(cdp)
    }

    /// Send one command and wait for its response, dropping events in between.
    pub fn call_timeout(&mut self, session: Option<&str>, method: &str, params: Value, timeout: Duration) -> Result<Value, AdlError> {
        self.next_id += 1;
        let id = self.next_id;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws.send(Message::text(msg.to_string())).map_err(transport)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(AdlError::new(ErrorCode::OutcomeUnknown, format!("{method}: the browser did not answer within {} s", timeout.as_secs())));
            }
            self.ws.get_mut().set_read_timeout(Some(left)).ok();
            let m = match self.ws.read() {
                Ok(m) => m,
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(e) => return Err(transport(e)),
            };
            let text = match m {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                Message::Close(_) => return Err(transport("browser closed the connection")),
                _ => continue,
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                let code = err["code"].as_i64().unwrap_or(0);
                let text = err["message"].as_str().unwrap_or("error");
                let ec = if text.contains("No node") || text.contains("Could not find node") || text.contains("No target") { ErrorCode::TargetStale } else { ErrorCode::Invalid };
                return Err(AdlError::new(ec, format!("{method}: {text}")).with_details(json!({"cdp_code": code})));
            }
            return Ok(v.get("result").cloned().unwrap_or(json!({})));
        }
    }

    pub fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Result<Value, AdlError> {
        self.call_timeout(session, method, params, CALL_TIMEOUT)
    }

    /// Page targets (tabs), the browser's own pages left out.
    pub fn pages(&mut self) -> Result<Vec<Value>, AdlError> {
        let r = self.call(None, "Target.getTargets", json!({}))?;
        Ok(r["targetInfos"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t["type"] == "page")
            .filter(|t| !t["url"].as_str().unwrap_or("").starts_with("devtools://"))
            .collect())
    }

    /// The flattened session for a page, attaching on first use.
    pub fn session(&mut self, target: &str) -> Result<String, AdlError> {
        if let Some(s) = self.sessions.get(target) {
            return Ok(s.clone());
        }
        let r = self.call(None, "Target.attachToTarget", json!({"targetId": target, "flatten": true})).map_err(|e| {
            if is_transport(&e) { e } else { AdlError::not_found(format!("no browser page {target}; list them with browser_pages")) }
        })?;
        let s = r["sessionId"].as_str().ok_or_else(|| AdlError::internal("attachToTarget returned no session"))?.to_string();
        self.sessions.insert(target.to_string(), s.clone());
        Ok(s)
    }

    /// Call a method on a page, re-attaching once if the session went away.
    pub fn page_call(&mut self, target: &str, method: &str, params: Value) -> Result<Value, AdlError> {
        self.page_call_timeout(target, method, params, CALL_TIMEOUT)
    }

    pub fn page_call_timeout(&mut self, target: &str, method: &str, params: Value, timeout: Duration) -> Result<Value, AdlError> {
        let s = self.session(target)?;
        match self.call_timeout(Some(&s), method, params.clone(), timeout) {
            Err(e) if e.message.contains("session") || e.message.contains("Session") => {
                self.sessions.remove(target);
                let s = self.session(target)?;
                self.call_timeout(Some(&s), method, params, timeout)
            }
            r => r,
        }
    }

    pub fn forget(&mut self, target: &str) {
        self.sessions.remove(target);
    }
}
