//! Fleet screens: the owner watches this machine's desktop on w3bs.org/fleet/<id>/screen.
//!
//! Off until someone on the machine runs `w3bs fleet screen on` (or joins with `--screen`).
//! Then `w3bs fleet agent` keeps one outbound WebSocket to the fleet server
//! (`/api/fleet/listen`). When the owner opens the screen page, the server sends a session
//! id; the agent dials `/api/fleet/screen/<id>`, connects it to the local VNC server
//! (127.0.0.1:5900 unless `--vnc` says otherwise; `unix:/path` for a Unix socket, which is how
//! `w3bs fleet desktop` shares its container) and copies bytes both ways. Nothing on the
//! machine listens to the network.
//!
//! In `view` mode the agent follows the RFB stream and drops every message that could change
//! the machine (keys, pointer, clipboard, resize, power), so a page that skips its own
//! view-only switch still cannot type. `control` passes the stream through untouched.

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, Message, client::IntoClientRequest};

pub const DEFAULT_VNC: &str = "127.0.0.1:5900";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_EVERY: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    View,
    Control,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "view" => Some(Self::View),
            "control" => Some(Self::Control),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Control => "control",
        }
    }
}

/// `screen` in fleet.json.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub mode: Mode,
    pub vnc: String,
}

impl Screen {
    pub fn to_json(&self) -> Value {
        json!({ "mode": self.mode.as_str(), "vnc": self.vnc })
    }
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self { mode: Mode::parse(v["mode"].as_str()?)?, vnc: v["vnc"].as_str().filter(|s| !s.is_empty()).unwrap_or(DEFAULT_VNC).to_owned() })
    }
}

/// `host:port` (the port required: VNC has no default the user should be guessing at), or
/// `unix:/absolute/path` for a Unix socket.
pub fn check_addr(addr: &str) -> Result<(), String> {
    if let Some(path) = addr.strip_prefix("unix:") {
        return if path.starts_with('/') { Ok(()) } else { Err(format!("{addr:?}: a Unix socket needs an absolute path, e.g. unix:/run/vnc.sock")) };
    }
    let (host, port) = addr.rsplit_once(':').ok_or_else(|| format!("{addr:?} needs a port, e.g. {DEFAULT_VNC}"))?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return Err(format!("{addr:?} is not host:port, e.g. {DEFAULT_VNC}"));
    }
    Ok(())
}

// ------------------------------------------------------------------ reachability

static PROBE: Mutex<Option<(Instant, String, bool)>> = Mutex::new(None);

/// Whether something accepts connections on the VNC address. Checked at most every two minutes so
/// the VNC server is not poked on every heartbeat; `forget_probe` forces the next check.
pub fn reachable(addr: &str) -> bool {
    let mut cache = PROBE.lock().unwrap();
    if let Some((at, a, ok)) = cache.as_ref()
        && a == addr
        && at.elapsed() < PROBE_EVERY
    {
        return *ok;
    }
    use std::net::ToSocketAddrs;
    #[cfg(unix)]
    if let Some(path) = addr.strip_prefix("unix:") {
        let ok = std::os::unix::net::UnixStream::connect(path).is_ok();
        *cache = Some((Instant::now(), addr.to_owned(), ok));
        return ok;
    }
    let ok = addr.to_socket_addrs().ok().into_iter().flatten().any(|sa| std::net::TcpStream::connect_timeout(&sa, Duration::from_secs(1)).is_ok());
    *cache = Some((Instant::now(), addr.to_owned(), ok));
    ok
}

pub fn forget_probe() {
    *PROBE.lock().unwrap() = None;
}

/// What the heartbeat reports about the screen.
pub fn info(s: &Screen) -> Value {
    json!({ "mode": s.mode.as_str(), "vnc": s.vnc, "reachable": reachable(&s.vnc) })
}

// ------------------------------------------------------------------ RFB view-only gate

/// RFB security types a view-only session can follow: None and VNC password. Anything else
/// (VeNCrypt, TLS, ...) wraps the stream and would hide the messages the gate must see.
const FOLLOWABLE: &[u8] = &[1, 2];
const MAX_CUT_TEXT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ServerAt {
    Version,
    Security,
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ClientAt {
    Version,
    /// Waiting for the server's security offer before the client's next bytes make sense.
    Security,
    Auth(usize),
    ClientInit,
    Messages,
}

/// Follows one RFB session (RFC 6143) and, in view mode, removes input from the viewer.
#[derive(Debug)]
pub struct Gate {
    view_only: bool,
    server: ServerAt,
    client: ClientAt,
    sbuf: Vec<u8>,
    cbuf: Vec<u8>,
    server_minor: Option<u32>,
    /// Negotiated: 3 (3.3), 7 or 8.
    minor: Option<u32>,
    offered: Option<Vec<u8>>,
    /// Chosen by the server under 3.3.
    chosen: Option<u8>,
    pub dropped: u64,
}

fn version(b: &[u8]) -> Result<u32, String> {
    let s = std::str::from_utf8(b).map_err(|_| "not an RFB server".to_owned())?;
    let rest = s.strip_prefix("RFB 003.").and_then(|r| r.strip_suffix('\n')).ok_or_else(|| format!("not an RFB greeting: {s:?}"))?;
    rest.parse::<u32>().map_err(|_| format!("not an RFB greeting: {s:?}"))
}

impl Gate {
    pub fn new(view_only: bool) -> Self {
        Self { view_only, server: ServerAt::Version, client: ClientAt::Version, sbuf: Vec::new(), cbuf: Vec::new(), server_minor: None, minor: None, offered: None, chosen: None, dropped: 0 }
    }

    /// Bytes from the VNC server, to forward to the viewer.
    pub fn server_bytes(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
        if !self.view_only || self.server == ServerAt::Pass {
            return Ok(data.to_vec());
        }
        self.sbuf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            match self.server {
                ServerAt::Version => {
                    if self.sbuf.len() < 12 {
                        return Ok(out);
                    }
                    let v: Vec<u8> = self.sbuf.drain(..12).collect();
                    self.server_minor = Some(version(&v)?);
                    out.extend(v);
                    self.server = ServerAt::Security;
                }
                ServerAt::Security => {
                    if self.sbuf.is_empty() {
                        return Ok(out);
                    }
                    // The server speaks only after the client chose a version.
                    let minor = self.minor.ok_or("the VNC server spoke out of turn")?;
                    if minor >= 7 {
                        let n = self.sbuf[0] as usize;
                        if n == 0 {
                            self.server = ServerAt::Pass; // failure reason follows
                            continue;
                        }
                        if self.sbuf.len() < 1 + n {
                            return Ok(out);
                        }
                        let offer: Vec<u8> = self.sbuf.drain(..1 + n).skip(1).filter(|t| FOLLOWABLE.contains(t)).collect();
                        if offer.is_empty() {
                            return Err("the VNC server needs a security type a view-only session cannot follow; allow no password or a VNC password (or share with --control)".into());
                        }
                        out.push(offer.len() as u8);
                        out.extend(&offer);
                        self.offered = Some(offer);
                    } else {
                        if self.sbuf.len() < 4 {
                            return Ok(out);
                        }
                        let t: Vec<u8> = self.sbuf.drain(..4).collect();
                        let t32 = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
                        if t32 != 0 && !FOLLOWABLE.contains(&(t32 as u8)) || t32 > 255 {
                            return Err(format!("the VNC server chose security type {t32}, which a view-only session cannot follow"));
                        }
                        if t32 != 0 {
                            self.chosen = Some(t32 as u8);
                        }
                        out.extend(t);
                    }
                    self.server = ServerAt::Pass;
                }
                ServerAt::Pass => {
                    out.append(&mut self.sbuf);
                    return Ok(out);
                }
            }
        }
    }

    /// Bytes from the viewer, to forward to the VNC server.
    pub fn viewer_bytes(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
        if !self.view_only {
            return Ok(data.to_vec());
        }
        self.cbuf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            let b = &self.cbuf;
            match self.client {
                ClientAt::Version => {
                    if b.len() < 12 {
                        return Ok(out);
                    }
                    let v: Vec<u8> = self.cbuf.drain(..12).collect();
                    let client = version(&v)?;
                    let server = self.server_minor.ok_or("the viewer spoke out of turn")?;
                    let m = client.min(server);
                    self.minor = Some(if m >= 8 { 8 } else if m == 7 { 7 } else { 3 });
                    out.extend(v);
                    self.client = ClientAt::Security;
                }
                ClientAt::Security => {
                    if b.is_empty() {
                        return Ok(out);
                    }
                    let chosen = if self.minor >= Some(7) {
                        let offered = self.offered.as_ref().ok_or("the viewer spoke out of turn")?;
                        let t = self.cbuf.remove(0);
                        if !offered.contains(&t) {
                            return Err(format!("the viewer chose security type {t}, which was not offered"));
                        }
                        out.push(t);
                        t
                    } else {
                        self.chosen.ok_or("the viewer spoke out of turn")?
                    };
                    self.client = if chosen == 2 { ClientAt::Auth(16) } else { ClientAt::ClientInit };
                }
                ClientAt::Auth(n) => {
                    if b.len() < n {
                        return Ok(out);
                    }
                    out.extend(self.cbuf.drain(..n));
                    self.client = ClientAt::ClientInit;
                }
                ClientAt::ClientInit => {
                    if b.is_empty() {
                        return Ok(out);
                    }
                    out.push(self.cbuf.remove(0));
                    self.client = ClientAt::Messages;
                }
                ClientAt::Messages => {
                    let Some((len, keep)) = client_message(b)? else { return Ok(out) };
                    let msg: Vec<u8> = self.cbuf.drain(..len).collect();
                    if keep {
                        out.extend(msg);
                    } else {
                        self.dropped += 1;
                    }
                }
            }
        }
    }
}

/// Length of the viewer's next message and whether a view-only session forwards it; `None`
/// until the whole message is buffered.
fn client_message(b: &[u8]) -> Result<Option<(usize, bool)>, String> {
    let need = |n: usize| b.len() >= n;
    let u16_at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]) as usize;
    let Some(&t) = b.first() else { return Ok(None) };
    let (len, keep) = match t {
        0 => (20, true),                     // SetPixelFormat
        2 if need(4) => (4 + 4 * u16_at(2), true), // SetEncodings
        3 => (10, true),                     // FramebufferUpdateRequest
        4 => (8, false),                     // KeyEvent
        5 => (6, false),                     // PointerEvent
        6 if need(8) => {
            // ClientCutText; a negative length is the extended clipboard format.
            let n = i32::from_be_bytes([b[4], b[5], b[6], b[7]]).unsigned_abs() as usize;
            if n > MAX_CUT_TEXT {
                return Err("the viewer sent an oversized clipboard".into());
            }
            (8 + n, false)
        }
        150 => (10, true),                   // EnableContinuousUpdates
        248 if need(9) => (9 + b[8] as usize, true), // ClientFence
        251 => (4, false),                   // xvp: reboot, shutdown
        252 if need(8) => (8 + 16 * b[6] as usize, false), // SetDesktopSize
        253 if need(4) => (4 + u16_at(2), false), // gii input devices
        255 if need(2) => match b[1] {
            0 => (12, false),                // QEMU extended key event
            1 if need(4) => (if u16_at(2) == 2 { 10 } else { 4 }, true), // QEMU audio
            1 => return Ok(None),
            s => return Err(format!("the viewer sent an unknown QEMU message {s}")),
        },
        2 | 6 | 248 | 252 | 253 | 255 => return Ok(None),
        t => return Err(format!("the viewer sent an unknown RFB message type {t}")),
    };
    Ok(need(len).then_some((len, keep)))
}

// ------------------------------------------------------------------ WebSockets

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn ws_url(origin: &str, path: &str) -> String {
    let rest = origin.strip_prefix("https://").map(|r| format!("wss://{r}")).or_else(|| origin.strip_prefix("http://").map(|r| format!("ws://{r}")));
    format!("{}{path}", rest.unwrap_or_else(|| origin.to_owned()))
}

fn tls() -> Result<tokio_tungstenite::Connector, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_platform_verifier()
        .map_err(|e| e.to_string())?
        .with_no_client_auth();
    Ok(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
}

/// Why a WebSocket did not open; `Unauthorized` means the machine left the fleet.
#[derive(Debug)]
pub enum DialError {
    Unauthorized,
    Other(String),
}

async fn dial(origin: &str, path: &str, key: &str) -> Result<Ws, DialError> {
    let mut req = ws_url(origin, path).into_client_request().map_err(|e| DialError::Other(e.to_string()))?;
    req.headers_mut().insert("authorization", format!("Bearer {key}").parse().map_err(|_| DialError::Other("bad agent key".into()))?);
    let connector = tls().map_err(DialError::Other)?;
    let dialing = tokio_tungstenite::connect_async_tls_with_config(req, None, false, Some(connector));
    match tokio::time::timeout(Duration::from_secs(20), dialing).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(tungstenite::Error::Http(r))) if r.status() == 401 => Err(DialError::Unauthorized),
        Ok(Err(e)) => Err(DialError::Other(e.to_string())),
        Err(_) => Err(DialError::Other("timed out".into())),
    }
}

/// The standing connection: waits for session ids and starts each session. Returns only
/// when the server says this machine is no longer in a fleet; reconnects otherwise.
/// Dropping the future (screen turned off) ends every session it started.
pub async fn listen(origin: String, key: String, screen: Screen) {
    let mut backoff = 5u64;
    let mut sessions = tokio::task::JoinSet::new();
    loop {
        match dial(&origin, "/api/fleet/listen", &key).await {
            Ok(mut ws) => {
                backoff = 5;
                eprintln!("w3bs fleet agent: sharing the screen ({}) from {}", screen.mode.as_str(), screen.vnc);
                loop {
                    tokio::select! {
                        msg = ws.next() => match msg {
                            Some(Ok(Message::Text(t))) => {
                                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                                if v["type"] == "screen"
                                    && let Some(id) = v["session"].as_str().filter(|s| s.starts_with("s_") && s.len() < 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
                                {
                                    eprintln!("w3bs fleet agent: screen session {id} opened by the fleet owner");
                                    sessions.spawn(session(origin.clone(), key.clone(), screen.clone(), id.to_owned()));
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => break,
                        },
                        Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
                    }
                }
            }
            Err(DialError::Unauthorized) => return,
            Err(DialError::Other(e)) => eprintln!("w3bs fleet agent: screen listener: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

/// A connection to the VNC server at `host:port` or `unix:/path`.
async fn connect_vnc(addr: &str) -> std::io::Result<Box<dyn Io>> {
    #[cfg(unix)]
    if let Some(path) = addr.strip_prefix("unix:") {
        return Ok(Box::new(tokio::net::UnixStream::connect(path).await?));
    }
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let _ = tcp.set_nodelay(true);
    Ok(Box::new(tcp))
}

/// One viewer: the session WebSocket joined to a fresh connection to the VNC server.
async fn session(origin: String, key: String, screen: Screen, id: String) {
    let vnc = tokio::time::timeout(CONNECT_TIMEOUT, connect_vnc(&screen.vnc)).await;
    let mut ws = match dial(&origin, &format!("/api/fleet/screen/{id}"), &key).await {
        Ok(ws) => ws,
        Err(e) => return eprintln!("w3bs fleet agent: screen session {id}: {e:?}"),
    };
    let conn = match vnc {
        Ok(Ok(conn)) => conn,
        failed => {
            forget_probe();
            let why = match failed {
                Ok(Err(e)) => e.to_string(),
                _ => "timed out".into(),
            };
            let error = format!("No VNC server answers on {} ({why}). See w3bs.org/docs/fleet-screen.md", screen.vnc);
            eprintln!("w3bs fleet agent: screen session {id}: {error}");
            let _ = ws.send(Message::Text(json!({ "error": error }).to_string().into())).await;
            let _ = ws.close(None).await;
            return;
        }
    };
    let (mut vnc_rx, mut vnc_tx) = tokio::io::split(conn);
    let (mut ws_tx, mut ws_rx) = ws.split();
    let gate = Arc::new(Mutex::new(Gate::new(screen.mode == Mode::View)));
    let (fail_tx, mut fail_rx) = tokio::sync::mpsc::channel::<String>(2);

    // VNC server -> viewer. Owns the WebSocket sink; errors from either side end here.
    let g = gate.clone();
    let to_viewer = async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            tokio::select! {
                n = vnc_rx.read(&mut buf) => {
                    let n = match n { Ok(0) | Err(_) => break, Ok(n) => n };
                    let out = g.lock().unwrap().server_bytes(&buf[..n]);
                    match out {
                        Ok(out) if out.is_empty() => {}
                        Ok(out) => if ws_tx.send(Message::Binary(out.into())).await.is_err() { break },
                        Err(why) => {
                            let _ = ws_tx.send(Message::Text(json!({ "error": why }).to_string().into())).await;
                            break;
                        }
                    }
                }
                Some(why) = fail_rx.recv() => {
                    let _ = ws_tx.send(Message::Text(json!({ "error": why }).to_string().into())).await;
                    break;
                }
            }
        }
        let _ = ws_tx.close().await;
    };
    // Viewer -> VNC server.
    let g = gate.clone();
    let to_vnc = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let Message::Binary(data) = msg else {
                if matches!(msg, Message::Close(_)) {
                    break;
                }
                continue;
            };
            let out = g.lock().unwrap().viewer_bytes(&data);
            match out {
                Ok(out) => {
                    if !out.is_empty() && vnc_tx.write_all(&out).await.is_err() {
                        break;
                    }
                }
                Err(why) => {
                    let _ = fail_tx.send(why).await;
                    break;
                }
            }
        }
        let _ = vnc_tx.shutdown().await;
    };
    let started = Instant::now();
    tokio::select! {
        _ = to_viewer => {}
        _ = to_vnc => {}
    }
    let dropped = gate.lock().unwrap().dropped;
    eprintln!(
        "w3bs fleet agent: screen session {id} ended after {}s{}",
        started.elapsed().as_secs(),
        if dropped > 0 { format!("; {dropped} input messages dropped (view only)") } else { String::new() }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handshake(g: &mut Gate, security: &[u8], pick: u8) -> Vec<u8> {
        assert_eq!(g.server_bytes(b"RFB 003.008\n").unwrap(), b"RFB 003.008\n");
        assert_eq!(g.viewer_bytes(b"RFB 003.008\n").unwrap(), b"RFB 003.008\n");
        let mut offer = vec![security.len() as u8];
        offer.extend(security);
        let shown = g.server_bytes(&offer).unwrap();
        let mut out = g.viewer_bytes(&[pick]).unwrap();
        if pick == 2 {
            assert_eq!(g.server_bytes(&[7; 16]).unwrap(), vec![7; 16], "challenge passes");
            out.extend(g.viewer_bytes(&[9; 16]).unwrap());
        }
        assert_eq!(g.server_bytes(&[0, 0, 0, 0]).unwrap(), vec![0, 0, 0, 0], "SecurityResult passes");
        out.extend(g.viewer_bytes(&[1]).unwrap()); // ClientInit, shared
        assert_eq!(g.client, ClientAt::Messages);
        shown
    }

    #[test]
    fn view_only_drops_input_and_keeps_updates() {
        let mut g = Gate::new(true);
        // Offer filtered to what the gate can follow.
        assert_eq!(handshake(&mut g, &[19, 2, 1, 30], 1), vec![2, 2, 1]);
        let update = [3, 1, 0, 0, 0, 0, 7, 128, 4, 56];
        let key = [4, 1, 0, 0, 0, 0, 0, 0x61];
        let pointer = [5, 1, 0, 10, 0, 20];
        let mut cut = vec![6, 0, 0, 0, 0, 0, 0, 3];
        cut.extend(b"abc");
        let mut stream = Vec::new();
        for m in [&update[..], &key, &pointer, &cut, &update] {
            stream.extend_from_slice(m);
        }
        // Split at an awkward place: messages straddle reads.
        let (a, b) = stream.split_at(13);
        let mut out = g.viewer_bytes(a).unwrap();
        out.extend(g.viewer_bytes(b).unwrap());
        let mut want = update.to_vec();
        want.extend(update);
        assert_eq!(out, want);
        assert_eq!(g.dropped, 3);
        // SetEncodings with two encodings, xvp (power), SetDesktopSize, QEMU key: only encodings pass.
        let enc = [2, 0, 0, 2, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21];
        let xvp = [251, 0, 1, 4];
        let mut resize = vec![252, 0, 7, 128, 4, 56, 1, 0];
        resize.extend([0u8; 16]);
        let qkey = [255, 0, 0, 1, 0, 0, 0, 0x61, 0, 0, 0, 30];
        let mut all = enc.to_vec();
        all.extend(xvp);
        all.extend(&resize);
        all.extend(qkey);
        assert_eq!(g.viewer_bytes(&all).unwrap(), enc.to_vec());
        assert_eq!(g.dropped, 6);
        // Server data after the handshake is untouched.
        assert_eq!(g.server_bytes(&[0, 0, 0, 1, 2, 3]).unwrap(), vec![0, 0, 0, 1, 2, 3]);
    }

    #[test]
    fn vnc_password_and_old_versions() {
        let mut g = Gate::new(true);
        handshake(&mut g, &[2], 2);
        assert_eq!(g.viewer_bytes(&[5, 0, 0, 1, 0, 1]).unwrap(), Vec::<u8>::new());

        // RFB 3.3: the server picks the type.
        let mut g = Gate::new(true);
        g.server_bytes(b"RFB 003.003\n").unwrap();
        g.viewer_bytes(b"RFB 003.003\n").unwrap();
        assert_eq!(g.server_bytes(&[0, 0, 0, 2]).unwrap(), vec![0, 0, 0, 2]);
        g.server_bytes(&[1; 16]).unwrap();
        assert_eq!(g.viewer_bytes(&[3; 16]).unwrap(), vec![3; 16]);
        assert_eq!(g.viewer_bytes(&[1]).unwrap(), vec![1]);
        assert_eq!(g.client, ClientAt::Messages);

        // Apple's 3.889 against a 3.8 viewer negotiates 3.8.
        let mut g = Gate::new(true);
        g.server_bytes(b"RFB 003.889\n").unwrap();
        g.viewer_bytes(b"RFB 003.008\n").unwrap();
        assert_eq!(g.server_bytes(&[2, 30, 2]).unwrap(), vec![1, 2]);
    }

    #[test]
    fn view_only_refuses_what_it_cannot_follow() {
        let mut g = Gate::new(true);
        g.server_bytes(b"RFB 003.008\n").unwrap();
        g.viewer_bytes(b"RFB 003.008\n").unwrap();
        assert!(g.server_bytes(&[1, 19]).unwrap_err().contains("--control"));

        let mut g = Gate::new(true);
        handshake(&mut g, &[1], 1);
        assert!(g.viewer_bytes(&[77, 0, 0]).unwrap_err().contains("unknown RFB message type 77"));

        let mut g = Gate::new(true);
        g.server_bytes(b"RFB 003.008\n").unwrap();
        g.viewer_bytes(b"RFB 003.008\n").unwrap();
        g.server_bytes(&[1, 1]).unwrap();
        assert!(g.viewer_bytes(&[2]).unwrap_err().contains("not offered"));

        assert!(Gate::new(true).server_bytes(b"HTTP/1.1 200\n").is_err());
    }

    #[test]
    fn control_passes_everything() {
        let mut g = Gate::new(false);
        assert_eq!(g.server_bytes(&[1, 19]).unwrap(), vec![1, 19]);
        assert_eq!(g.viewer_bytes(&[4, 1, 0, 0, 0, 0, 0, 0x61]).unwrap(), vec![4, 1, 0, 0, 0, 0, 0, 0x61]);
    }

    #[test]
    fn config_and_addresses() {
        let s = Screen { mode: Mode::View, vnc: DEFAULT_VNC.into() };
        assert_eq!(Screen::from_json(&s.to_json()), Some(s));
        assert_eq!(Screen::from_json(&json!({"mode": "control"})).unwrap().vnc, DEFAULT_VNC);
        assert_eq!(Screen::from_json(&json!({"mode": "spy"})), None);
        assert!(check_addr("127.0.0.1:5900").is_ok());
        assert!(check_addr("[::1]:5901").is_ok());
        assert!(check_addr("localhost").is_err());
        assert!(check_addr(":5900").is_err());
        assert!(check_addr("unix:/home/a/.local/share/w3bs/desktop/vnc/vnc.sock").is_ok());
        assert!(check_addr("unix:vnc.sock").is_err());
        assert_eq!(ws_url("https://w3bs.org", "/api/fleet/listen"), "wss://w3bs.org/api/fleet/listen");
        assert_eq!(ws_url("http://localhost:3000", "/x"), "ws://localhost:3000/x");
    }
}
