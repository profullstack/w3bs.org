//! End to end through the real binary: daemon (mock adapter), terminal control, CLI and MCP.
//! Unix transport (Linux, macOS); the Windows named-pipe variant lives with the Windows port.
#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_w3bs-desktop");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        let d = self.dir.path();
        c.env("W3BS_DESKTOP_RUNTIME_DIR", d.join("run")).env("W3BS_DESKTOP_CONFIG_DIR", d.join("config")).env("W3BS_DESKTOP_STATE_DIR", d.join("state")).env_remove("W3BS_DESKTOP_TOKEN").env_remove("W3BS_DESKTOP_AGENT");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).stdin(Stdio::null()).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let o = self.run(&a);
        let v = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&o.stdout), "stderr": String::from_utf8_lossy(&o.stderr)}));
        (o.status.code().unwrap_or(-1), v)
    }
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(path: &Path) {
    let t = Instant::now();
    while !path.exists() {
        assert!(t.elapsed() < Duration::from_secs(10), "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_state(env: &Env, want: &str) {
    let t = Instant::now();
    loop {
        let (_, v) = env.json(&["status"]);
        if v["state"] == want {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "state never became {want}: {v}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

struct Mcp {
    _child: Kill,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    id: u64,
}

impl Mcp {
    fn req(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params})).unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], self.id);
        v
    }
    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.req("tools/call", json!({"name": name, "arguments": args}))["result"].clone()
    }
}

#[test]
fn daemon_control_cli_and_mcp() {
    let env = Env { dir: tempfile::tempdir().unwrap() };
    let _daemon = Kill(env.cmd().args(["daemon", "--adapter", "mock"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait_for(&env.dir.path().join("run/control.sock"));

    // Sockets are private.
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(env.dir.path().join("run/agent.sock")).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "agent socket is not private: {mode:o}");

    let (code, st) = env.json(&["status"]);
    assert_eq!((code, st["state"].as_str()), (0, Some("disabled")), "fresh start is Disabled");

    let (code, added) = env.json(&["agents", "add", "claude-code", "--preset", "broad"]);
    assert_eq!(code, 0, "{added}");
    let token_file = added["token_file"].as_str().unwrap().to_string();
    assert_eq!(std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o077, 0);

    // Agents are refused while disabled; CLI enable without a native control is blocked.
    let (code, v) = env.json(&["windows", "list", "--agent", "claude-code"]);
    assert_eq!((code, v["error"]["code"].as_str()), (4, Some("disabled")));
    let (code, v) = env.json(&["enable"]);
    assert_eq!((code, v["error"]["code"].as_str()), (4, Some("blocked")), "{v}");

    // The terminal native control enables (it receives the code itself).
    let mut ctl = env.cmd().arg("control").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let mut ctl_in = ctl.stdin.take().unwrap();
    let ctl = Kill(ctl);
    std::thread::sleep(Duration::from_millis(200));
    writeln!(ctl_in, "enable").unwrap();
    wait_state(&env, "enabled");

    // Drive prompt: the agent's first input waits for the person, who allows it.
    let click = env
        .cmd()
        .env("W3BS_DESKTOP_DRIVE_WAIT_S", "20")
        .args(["--json", "mouse", "click", "--agent", "claude-code", "--x", "50", "--y", "50", "--window", "w-editor"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let t = Instant::now();
    let req = loop {
        let (_, v) = env.json(&["drive", "list"]);
        if let Some(d) = v.as_array().and_then(|a| a.iter().find(|d| d["status"] == "pending")) {
            break d.clone();
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no drive prompt appeared: {v}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(req["agent_name"], "claude-code");
    assert_eq!(env.json(&["status"]).1["pending_drive_requests"], 1);
    let (code, v) = env.json(&["drive", "allow", req["id"].as_str().unwrap()]);
    assert_eq!((code, v["status"].as_str()), (0, Some("allowed")), "{v}");
    let out = click.wait_with_output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(v["outcome"], "completed", "{v} {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("waiting for the person at the desktop"));

    // CLI input acquires and releases the lease itself.
    let (code, v) = env.json(&["mouse", "click", "--agent", "claude-code", "--x", "50", "--y", "50", "--window", "w-editor"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["outcome"], "completed");
    let txt = env.dir.path().join("t.txt");
    std::fs::write(&txt, "typed from a file").unwrap();
    let (code, v) = env.json(&["keyboard", "type", "--agent", "claude-code", "--window", "w-editor", "--from-file", txt.to_str().unwrap()]);
    assert_eq!(code, 0, "{v}");
    let (code, _) = env.json(&["keyboard", "type", "--agent", "claude-code", "--window", "w-editor"]);
    assert_eq!(code, 2, "text is never taken from arguments");
    let shot = env.dir.path().join("s.png");
    let (code, v) = env.json(&["screen", "capture", "--agent", "claude-code", "--display", "DP-1", "--output", shot.to_str().unwrap()]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(&std::fs::read(&shot).unwrap()[1..4], b"PNG");

    // A second agent, registered while this process is still a plain human shell.
    let (code, v) = env.json(&["agents", "add", "codex", "--preset", "broad"]);
    assert_eq!(code, 0, "{v}");

    // MCP over stdio.
    let mut child = env.cmd().args(["mcp", "serve", "--stdio", "--agent", "claude-code"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let mut mcp = Mcp { _child: Kill(child), stdin, out, id: 0 };
    let init = mcp.req("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "w3bs-desktop");
    let tools = mcp.req("tools/list", json!({}));
    let names: Vec<String> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"mouse_click".to_string()) && names.contains(&"keyboard_type".to_string()));
    assert!(!names.iter().any(|n| n.contains("enable") || n.contains("grant") || n == "approve"), "no self-escalation tools: {names:?}");

    let r = mcp.tool("mouse_click", json!({"at": {"space": "desktop", "x": 10, "y": 10}}));
    assert_eq!(r["isError"], true, "input before task_begin fails");
    let r = mcp.tool("task_begin", json!({"description": "e2e"}));
    assert_eq!(r["isError"], false, "{r}");
    let r = mcp.tool("desktop_windows", json!({}));
    assert!(r["content"][0]["text"].as_str().unwrap().contains("w-editor"));
    let r = mcp.tool("keyboard_type", json!({"window": "w-editor", "text": " and from MCP"}));
    assert_eq!(r["isError"], false, "{r}");
    let r = mcp.tool("keyboard_shortcut", json!({"window": "w-browser", "keys": "Ctrl+S"}));
    assert_eq!(r["isError"], true, "the browser is not focused, so the shortcut is refused: {r}");
    let r = mcp.tool("desktop_screenshot", json!({"display": "DP-2"}));
    assert_eq!(r["content"][0]["type"], "image");
    let cap: Value = serde_json::from_str(r["content"][1]["text"].as_str().unwrap()).unwrap();
    let r = mcp.tool("mouse_move", json!({"to": {"space": "capture", "capture": cap["capture"], "x": 100, "y": 100}}));
    assert_eq!(r["isError"], false, "{r}");

    // This test process launched the MCP server, so it is now an agent root: its children may
    // not raise access through the control socket.
    let (code, v) = env.json(&["agents", "add", "sneaky", "--preset", "broad"]);
    assert_eq!(code, 4, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("agent"), "{v}");
    // Another agent cannot take the lease while MCP holds it.
    let (code, v) = env.json(&["mouse", "click", "--agent", "codex", "--x", "5", "--y", "5"]);
    assert_eq!((code, v["error"]["code"].as_str()), (5, Some("busy")), "{v}");

    // Disable is always allowed, even from an agent's process tree, and stops MCP work.
    let (code, v) = env.json(&["disable"]);
    assert_eq!(code, 0, "{v}");
    assert!(v["tasks_cancelled"].as_u64().unwrap() >= 1);
    let r = mcp.tool("mouse_click", json!({"at": {"space": "desktop", "x": 10, "y": 10}}));
    assert!(r["content"][0]["text"].as_str().unwrap().starts_with("disabled"), "{r}");

    // Audit has metadata and no typed text.
    let (_, audit) = env.json(&["audit", "export"]);
    let s = audit.to_string();
    assert!(s.contains("\"input\"") && s.contains("disable"));
    assert!(!s.contains("typed from a file") && !s.contains("from MCP"));
    drop(ctl);
}

/// One HTTP/1.1 request to the broker's MCP endpoint; returns (status, headers, body).
fn http(port: u16, method: &str, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
    http_at(port, method, "/mcp", headers, body)
}

fn http_at(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        req.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
}

#[test]
fn mcp_over_http_in_the_daemon() {
    let env = Env { dir: tempfile::tempdir().unwrap() };
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    // Non-loopback addresses are refused outright.
    let refused = env.run(&["daemon", "--adapter", "mock", "--mcp-http", "0.0.0.0:7801"]);
    assert_eq!(refused.status.code(), Some(2), "{}", String::from_utf8_lossy(&refused.stderr));
    let _daemon = Kill(env.cmd().args(["daemon", "--adapter", "mock", "--mcp-http", &addr]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait_for(&env.dir.path().join("run/control.sock"));
    let (code, added) = env.json(&["agents", "add", "claude-code", "--preset", "broad"]);
    assert_eq!(code, 0, "{added}");
    let token = std::fs::read_to_string(added["token_file"].as_str().unwrap()).unwrap();
    let (_, other) = env.json(&["agents", "add", "codex", "--preset", "broad"]);
    let other_token = std::fs::read_to_string(other["token_file"].as_str().unwrap()).unwrap();
    let mut ctl = env.cmd().arg("control").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let mut ctl_in = ctl.stdin.take().unwrap();
    let _ctl = Kill(ctl);
    std::thread::sleep(Duration::from_millis(200));
    writeln!(ctl_in, "enable").unwrap();
    wait_state(&env, "enabled");
    // The person at this desktop chose "Always allow" (the prompt itself is checked above).
    assert_eq!(env.json(&["drive", "always-allow", "on"]).1["drive_always_allow"], true);

    let bearer = format!("Bearer {token}");
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#;
    // No token, a browser Origin, or a foreign Host are refused before anything else.
    assert_eq!(http(port, "POST", &[("Content-Type", "application/json")], init).0, 401);
    assert_eq!(http(port, "POST", &[("Authorization", &bearer), ("Origin", "https://evil.example")], init).0, 403);
    assert_eq!(http(port, "POST", &[("Authorization", &bearer), ("Host", "evil.example")], init).0, 403);
    assert_eq!(http(port, "POST", &[("Authorization", "Bearer adl_nope")], init).0, 401);
    assert_eq!(http(port, "GET", &[("Authorization", &bearer)], "").0, 405);

    let (code, head, body) = http(port, "POST", &[("Authorization", &bearer), ("Content-Type", "application/json")], init);
    assert_eq!(code, 200, "{head}\n{body}");
    let sid = header(&head, "mcp-session-id").expect("session id").to_string();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["serverInfo"]["name"], "w3bs-desktop");
    let call = |b: &str| http(port, "POST", &[("Authorization", &bearer), ("Mcp-Session-Id", &sid)], b);
    assert_eq!(call(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).0, 202);
    let (code, _, body) = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    assert_eq!(code, 200);
    let tools: Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"mouse_click"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("enable") || n.contains("grant") || *n == "approve"), "no self-escalation tools: {names:?}");
    let (_, _, body) = call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"task_begin","arguments":{"description":"http"}}}"#);
    assert!(body.contains("\"isError\":false"), "{body}");
    let (_, _, body) = call(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"keyboard_type","arguments":{"window":"w-editor","text":"over http"}}}"#);
    assert!(body.contains("\"isError\":false"), "{body}");
    // Another agent's token cannot use this session.
    let stolen = http(port, "POST", &[("Authorization", &format!("Bearer {other_token}")), ("Mcp-Session-Id", &sid)], r#"{"jsonrpc":"2.0","id":5,"method":"tools/list"}"#);
    assert_eq!(stolen.0, 404);
    // On Linux the HTTP client (this test process) is now an agent root: it cannot add agents.
    #[cfg(target_os = "linux")]
    {
        let (code, v) = env.json(&["agents", "add", "sneaky", "--preset", "broad"]);
        assert_eq!(code, 4, "{v}");
    }
    // Disable stops HTTP sessions too.
    env.json(&["disable"]);
    let (_, _, body) = call(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"mouse_click","arguments":{"at":{"space":"desktop","x":5,"y":5}}}}"#);
    assert!(body.contains("disabled"), "{body}");
    assert_eq!(http(port, "DELETE", &[("Authorization", &bearer), ("Mcp-Session-Id", &sid)], "").0, 204);
    assert_eq!(call(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#).0, 404);
}

/// The control app through the real daemon: a one-time link from the terminal control, a
/// session cookie, enable/disable and an agent grant from the app, and the rules around it.
#[test]
fn control_app_in_the_daemon() {
    let env = Env { dir: tempfile::tempdir().unwrap() };
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    // No --mcp-http: the control app starts the loopback listener itself, without /mcp.
    let _daemon = Kill(env.cmd().args(["daemon", "--adapter", "mock"]).env("W3BS_DESKTOP_CONTROL_ADDR", &addr).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait_for(&env.dir.path().join("run/control.sock"));
    let (code, _) = env.json(&["agents", "add", "helper", "--preset", "observe"]);
    assert_eq!(code, 0);

    // Not from a terminal (stdin is /dev/null here) and not the native control: refused.
    let (code, v) = env.json(&["control", "--open", "--no-browser"]);
    assert_eq!((code, v["error"]["code"].as_str()), (4, Some("denied")), "{v}");

    // The terminal native control mints the link over its own connection.
    let mut ctl = env.cmd().arg("control").env("W3BS_DESKTOP_OPENER", "true").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut ctl_in = ctl.stdin.take().unwrap();
    let ctl_out = BufReader::new(ctl.stdout.take().unwrap());
    let ctl = Kill(ctl);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in ctl_out.lines().map_while(Result::ok) {
            if let Some(u) = l.split("Opening the control app: ").nth(1) {
                let _ = tx.send(u.trim().to_string());
            }
        }
    });
    std::thread::sleep(Duration::from_millis(200));
    writeln!(ctl_in, "open").unwrap();
    let url = rx.recv_timeout(Duration::from_secs(10)).expect("terminal control printed the link");
    let prefix = format!("http://{addr}/control/login?t=");
    assert!(url.starts_with(&prefix), "{url}");
    let path = &url[format!("http://{addr}").len()..];
    // /mcp is off on this listener.
    assert_eq!(http(port, "POST", &[("Authorization", "Bearer x")], "{}").0, 404);

    let (code, head, _) = http_at(port, "GET", path, &[], "");
    assert_eq!(code, 303, "{head}");
    let cookie = header(&head, "set-cookie").unwrap().split(';').next().unwrap().to_string();
    let (code, head, _) = http_at(port, "GET", path, &[], "");
    assert_eq!(code, 303);
    assert!(header(&head, "set-cookie").is_none(), "link works once");
    let (code, _, body) = http_at(port, "GET", "/control/api/state", &[("Cookie", &cookie)], "");
    assert_eq!(code, 200, "{body}");
    let st: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(st["status"]["state"], "disabled");
    assert_eq!(st["settings"]["native_controls"], 1);
    let csrf = st["csrf"].as_str().unwrap().to_string();
    let origin = format!("http://{addr}");
    let act = |body: &str| http_at(port, "POST", "/control/api/action", &[("Cookie", &cookie), ("Origin", &origin), ("X-W3BS-CSRF", &csrf), ("Content-Type", "application/json")], body);

    // Enable from the app (the terminal control is the attached native control).
    let (code, _, body) = act(r#"{"action":"enable"}"#);
    assert_eq!(code, 200, "{body}");
    wait_state(&env, "enabled");
    // Grant change, same operation as `agents update --policy broad`.
    let (code, _, body) = act(r#"{"action":"agent_update","agent":"helper","preset":"broad","apps":[]}"#);
    assert_eq!(code, 200, "{body}");
    let (_, list) = env.json(&["agents", "list"]);
    let helper = list.as_array().unwrap().iter().find(|a| a["name"] == "helper").unwrap().clone();
    assert_eq!(helper["policy"]["mode"], "broad_desktop", "{helper}");
    assert!(helper["policy"]["capabilities"].as_array().unwrap().iter().any(|c| c == "pointer"));
    let (code, _, _) = act(r#"{"action":"agent_update","agent":"helper","preset":"restricted","apps":[]}"#);
    assert_eq!(code, 400, "restricted needs an app");

    // Closing the app (dropping its event stream) does not disable access.
    {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET /control/api/events HTTP/1.1\r\nHost: {addr}\r\nCookie: {cookie}\r\n\r\n").unwrap();
        let mut first = [0u8; 12];
        std::io::Read::read_exact(&mut s, &mut first).unwrap();
        assert_eq!(&first, b"HTTP/1.1 200");
    }
    std::thread::sleep(Duration::from_millis(300));
    let (_, v) = env.json(&["status"]);
    assert_eq!(v["state"], "enabled", "closing the app must not disable");

    // Quit and disable: disables and signs the app out.
    let (code, head, body) = act(r#"{"action":"quit"}"#);
    assert_eq!(code, 200, "{body}");
    assert!(header(&head, "set-cookie").unwrap().contains("Max-Age=0"));
    wait_state(&env, "disabled");
    assert_eq!(http_at(port, "GET", "/control/api/state", &[("Cookie", &cookie)], "").0, 401);

    let (_, audit) = env.json(&["audit", "export"]);
    let s = audit.to_string();
    for want in ["control.login", "terminal control", "signed_in", "quit from the control app", "agents.update"] {
        assert!(s.contains(want), "audit lacks {want}: {s}");
    }
    drop(ctl);
}
