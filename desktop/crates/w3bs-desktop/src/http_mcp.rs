//! MCP over HTTP inside the broker daemon (`daemon --mcp-http 127.0.0.1:7801`), so agents connect
//! to a long-running endpoint instead of spawning `mcp serve --stdio` per session.
//!
//! The same loopback listener serves the human control app under `/control` (see
//! `control_web`). The two never share credentials: `/mcp` only takes agent bearer tokens and
//! ignores cookies; `/control` refuses any `Authorization` header and only takes its own session
//! cookie. When the daemon runs without `--mcp-http`, opening the control app starts this
//! listener with `/mcp` switched off (404).
//!
//! Streamable HTTP transport, JSON responses only (no SSE stream): `POST /mcp` carries one
//! JSON-RPC message, `DELETE /mcp` ends a session. PRD section 10 rules apply:
//! - loopback only; a non-loopback address is refused at startup
//! - every request needs `Authorization: Bearer <agent token>`; a session is bound to the token
//!   that opened it
//! - `Host` must be the loopback address and port we listen on, and any `Origin` header is refused,
//!   so a web page cannot reach it through the browser (DNS rebinding, CSRF)
//! - each MCP session is an ordinary agent connection to the broker, so the same leases, grants,
//!   approvals and disable gate apply; the tool surface has no enable/grant/approve methods
//! - on Linux the agent app's pid is resolved from the TCP connection and registered as an agent
//!   root, so its shell commands cannot raise access on the control endpoint (same as stdio MCP)

use crate::mcp::Session;
use adl_broker::util::sha256_hex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;
const IDLE: Duration = Duration::from_secs(30 * 60);

struct Entry {
    session: Session,
    token_hash: String,
    last: Instant,
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Mutex<Entry>>>>>;

/// Parse and validate the listen address: loopback only.
pub fn parse_addr(s: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = s.parse().map_err(|e| format!("{s}: {e} (expected e.g. 127.0.0.1:7801)"))?;
    if !addr.ip().is_loopback() {
        return Err(format!("{s}: the MCP HTTP endpoint only listens on loopback (127.0.0.1 or [::1])"));
    }
    Ok(addr)
}

/// The address the listener is bound to, and whether it serves `/mcp`.
static LISTENING: Mutex<Option<(SocketAddr, bool)>> = Mutex::new(None);

pub fn listening() -> Option<(SocketAddr, bool)> {
    *LISTENING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The listener's address, starting it (control app only, no `/mcp`) on `default` when the
/// daemon was started without `--mcp-http`.
pub fn ensure_started(default: SocketAddr) -> std::io::Result<SocketAddr> {
    // Hold the lock across bind so two concurrent callers cannot both start one.
    let mut l = LISTENING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((a, _)) = *l {
        return Ok(a);
    }
    let a = bind_and_serve(default, false)?;
    *l = Some((a, false));
    Ok(a)
}

/// Start the endpoint on its own threads. Returns the bound address once the socket is bound.
pub fn spawn(addr: SocketAddr, mcp: bool) -> std::io::Result<SocketAddr> {
    let mut l = LISTENING.lock().unwrap_or_else(|e| e.into_inner());
    let a = bind_and_serve(addr, mcp)?;
    *l = Some((a, mcp));
    Ok(a)
}

fn bind_and_serve(addr: SocketAddr, mcp: bool) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let addr = listener.local_addr()?;
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    if mcp {
        eprintln!("  mcp:     http://{addr}/mcp (loopback, bearer token required)");
    }
    eprintln!("  app:     http://{addr}/control (open it with `w3bs desktop control --open` or the tray)");
    {
        let sessions = sessions.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            let stale: Vec<Arc<Mutex<Entry>>> = {
                let mut m = sessions.lock().unwrap();
                let ids: Vec<String> = m.iter().filter(|(_, e)| e.lock().map(|e| e.last.elapsed() > IDLE).unwrap_or(true)).map(|(k, _)| k.clone()).collect();
                ids.iter().filter_map(|k| m.remove(k)).collect()
            };
            for e in stale {
                if let Ok(mut e) = e.lock() {
                    e.session.close();
                }
            }
        });
    }
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let sessions = sessions.clone();
            std::thread::spawn(move || {
                let _ = handle(conn, addr, mcp, sessions);
            });
        }
    });
    Ok(addr)
}

pub(crate) struct Req {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub(crate) fn read_request(r: &mut BufReader<TcpStream>) -> std::io::Result<Option<Req>> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = r.by_ref().take((MAX_HEADER - head.len().min(MAX_HEADER)) as u64 + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(None);
        }
        if head.len() + line.len() > MAX_HEADER {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "headers too large"));
        }
        let end = line == b"\r\n" || line == b"\n";
        head.extend_from_slice(&line);
        if end {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
    let headers: HashMap<String, String> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    Ok(Some(Req { method, path, headers, body }))
}

fn respond(w: &mut TcpStream, status: &str, extra: &[(&str, String)], body: Option<&Value>) -> std::io::Result<()> {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n", payload.len());
    if body.is_some() {
        out.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in extra {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(&payload);
    w.write_all(out.as_bytes())?;
    w.flush()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle(conn: TcpStream, addr: SocketAddr, mcp: bool, sessions: Sessions) -> std::io::Result<()> {
    let peer = conn.peer_addr()?;
    conn.set_read_timeout(Some(Duration::from_secs(120)))?;
    let mut w = conn.try_clone()?;
    let mut r = BufReader::new(conn);
    while let Some(req) = read_request(&mut r)? {
        let keep = !req.headers.get("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));
        let route = req.path.split('?').next().unwrap_or_default();
        if route == "/control" || route.starts_with("/control/") {
            // The control app may take the connection over (its event stream).
            if crate::control_web::serve(&req, &mut w, addr, peer)? == crate::control_web::Next::Close {
                break;
            }
        } else if mcp {
            serve(&req, &mut w, addr, peer, &sessions)?;
        } else {
            respond(&mut w, "404 Not Found", &[], Some(&json!({"error": "MCP over HTTP is off; start the daemon with --mcp-http"})))?;
        }
        if !keep {
            break;
        }
    }
    Ok(())
}

pub(crate) fn host_ok(host: Option<&String>, addr: SocketAddr) -> bool {
    let port = addr.port();
    host.is_some_and(|h| [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")].iter().any(|ok| h.eq_ignore_ascii_case(ok)))
}

fn serve(req: &Req, w: &mut TcpStream, addr: SocketAddr, peer: SocketAddr, sessions: &Sessions) -> std::io::Result<()> {
    if req.path.split('?').next() != Some("/mcp") {
        return respond(w, "404 Not Found", &[], None);
    }
    if !host_ok(req.headers.get("host"), addr) {
        return respond(w, "403 Forbidden", &[], Some(&json!({"error": "Host must be the loopback address of this endpoint"})));
    }
    if req.headers.contains_key("origin") {
        return respond(w, "403 Forbidden", &[], Some(&json!({"error": "browser requests are refused"})));
    }
    let Some(token) = req.headers.get("authorization").and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "))).map(str::trim).filter(|t| !t.is_empty()) else {
        return respond(w, "401 Unauthorized", &[("WWW-Authenticate", "Bearer".into())], Some(&json!({"error": "send Authorization: Bearer <agent token>"})));
    };
    let token_hash = sha256_hex(token.as_bytes());
    let sid = req.headers.get("mcp-session-id").cloned();
    match req.method.as_str() {
        "DELETE" => {
            let Some(sid) = sid else { return respond(w, "400 Bad Request", &[], None) };
            let e = sessions.lock().unwrap().get(&sid).cloned();
            match e {
                Some(e) if e.lock().map(|e| e.token_hash == token_hash).unwrap_or(false) => {
                    sessions.lock().unwrap().remove(&sid);
                    if let Ok(mut e) = e.lock() {
                        e.session.close();
                    }
                    respond(w, "204 No Content", &[], None)
                }
                _ => respond(w, "404 Not Found", &[], None),
            }
        }
        "POST" => {
            let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
                return respond(w, "400 Bad Request", &[], Some(&rpc_error(Value::Null, -32700, "parse error")));
            };
            if !msg.is_object() {
                return respond(w, "400 Bad Request", &[], Some(&rpc_error(Value::Null, -32600, "send one JSON-RPC message per request")));
            }
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            if msg.get("method").and_then(Value::as_str) == Some("initialize") {
                let pid = client_pid(peer, addr);
                let mut session = match Session::open(token, json!({"client": "http", "client_pid": pid})) {
                    Ok(s) => s,
                    Err(e) => return respond(w, "401 Unauthorized", &[("WWW-Authenticate", "Bearer".into())], Some(&rpc_error(id, -32001, &e.to_string()))),
                };
                let resp = session.handle(&msg);
                let new_id = adl_broker::util::random_id("mcp", 16);
                sessions.lock().unwrap().insert(new_id.clone(), Arc::new(Mutex::new(Entry { session, token_hash, last: Instant::now() })));
                return match resp {
                    Some(v) => respond(w, "200 OK", &[("Mcp-Session-Id", new_id)], Some(&v)),
                    None => respond(w, "202 Accepted", &[("Mcp-Session-Id", new_id)], None),
                };
            }
            let Some(sid) = sid else {
                return respond(w, "400 Bad Request", &[], Some(&rpc_error(id, -32600, "missing Mcp-Session-Id; send initialize first")));
            };
            let e = sessions.lock().unwrap().get(&sid).cloned();
            let Some(e) = e else { return respond(w, "404 Not Found", &[], Some(&rpc_error(id, -32001, "unknown or expired session; initialize again"))) };
            let mut e = e.lock().unwrap_or_else(|p| p.into_inner());
            if e.token_hash != token_hash {
                return respond(w, "404 Not Found", &[], Some(&rpc_error(id, -32001, "unknown or expired session; initialize again")));
            }
            e.last = Instant::now();
            match e.session.handle(&msg) {
                Some(v) => respond(w, "200 OK", &[], Some(&v)),
                None => respond(w, "202 Accepted", &[], None),
            }
        }
        _ => respond(w, "405 Method Not Allowed", &[("Allow", "POST, DELETE".into())], None),
    }
}

/// The pid owning the client end of a loopback TCP connection (Linux: /proc/net/tcp + fds).
#[cfg(target_os = "linux")]
pub(crate) fn client_pid(peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    use std::net::IpAddr;
    let (IpAddr::V4(pip), IpAddr::V4(lip)) = (peer.ip(), local.ip()) else { return None };
    let hex = |ip: std::net::Ipv4Addr, port: u16| format!("{:08X}:{:04X}", u32::from_le_bytes(ip.octets()), port);
    let (want_local, want_remote) = (hex(pip, peer.port()), hex(lip, local.port()));
    let table = std::fs::read_to_string("/proc/net/tcp").ok()?;
    let inode = table.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 9 && f[1] == want_local && f[2] == want_remote).then(|| f[9].to_string())
    })?;
    let target = format!("socket:[{inode}]");
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|p| p.to_string_lossy() == target) {
                return Some(pid);
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn client_pid(_peer: SocketAddr, _local: SocketAddr) -> Option<u32> {
    // Not resolved on macOS/Windows yet: the session still authenticates by token, but the agent
    // app is not registered as an agent root (stdio MCP keeps that protection everywhere).
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only() {
        assert!(parse_addr("127.0.0.1:7801").is_ok());
        assert!(parse_addr("[::1]:7801").is_ok());
        assert!(parse_addr("0.0.0.0:7801").is_err());
        assert!(parse_addr("192.168.1.5:7801").is_err());
        assert!(parse_addr("nonsense").is_err());
    }

    #[test]
    fn host_check() {
        let a: SocketAddr = "127.0.0.1:7801".parse().unwrap();
        assert!(host_ok(Some(&"127.0.0.1:7801".into()), a));
        assert!(host_ok(Some(&"localhost:7801".into()), a));
        assert!(!host_ok(Some(&"evil.example:7801".into()), a));
        assert!(!host_ok(Some(&"127.0.0.1:80".into()), a));
        assert!(!host_ok(None, a));
    }
}
