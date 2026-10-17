//! End to end through the real binary on Windows: daemon (mock adapter), terminal control, CLI
//! and MCP over named pipes. The same scenario as `e2e.rs` (Unix sockets), with the Windows
//! checks: pipes are found by name, the agent-root lineage comes from Toolhelp32.
#![cfg(windows)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
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

/// Pipes have no file to wait for; wait until the broker answers.
fn wait_ready(env: &Env) {
    let t = Instant::now();
    while env.json(&["status"]).0 != 0 {
        assert!(t.elapsed() < Duration::from_secs(20), "broker never answered");
        std::thread::sleep(Duration::from_millis(50));
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
    wait_ready(&env);

    // A second broker for the same pipes is refused (first-instance ownership).
    let second = env.cmd().args(["daemon", "--adapter", "mock"]).stdin(Stdio::null()).output().unwrap();
    assert_ne!(second.status.code(), Some(0), "second broker started");
    assert!(String::from_utf8_lossy(&second.stderr).contains("already"), "{}", String::from_utf8_lossy(&second.stderr));

    let (code, st) = env.json(&["status"]);
    assert_eq!((code, st["state"].as_str()), (0, Some("disabled")), "fresh start is Disabled");

    let (code, added) = env.json(&["agents", "add", "claude-code", "--preset", "broad"]);
    assert_eq!(code, 0, "{added}");
    let token_file = added["token_file"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&token_file).starts_with(env.dir.path().join("config")), "{token_file}");

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
    // The person at this desktop chose "Always allow" (the prompt itself is checked in e2e.rs).
    assert_eq!(env.json(&["drive", "always-allow", "on"]).1["drive_always_allow"], true);

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
