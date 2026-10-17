//! `w3bs desktop control`: a terminal native control, for sessions without a tray or panel
//! extension and as the reference implementation of the native-control protocol.
//!
//! It holds a native-control subscription, so the broker treats it like the tray: enable codes
//! are shown here, and closing it while access is enabled disables the layer.

use crate::paths;
use adl_schema::rpc::{Request, Response};
use adl_schema::SCHEMA_VERSION;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::sync::mpsc;

const HELP: &str = "commands: enable | disable | stop | confirm | approve <id> | deny <id> | approvals | allow | deny | postpone [minutes] | always (an agent asking to drive; add an id to pick one) | always off | tasks | status | open (the control app) | quit (quitting disables access)";

fn print_state(v: &Value) {
    let st = v["state"].as_str().unwrap_or("?");
    let icon = match st {
        "enabled" => "[ON ]",
        "disabled" => "[OFF]",
        "enabling" | "stopping" => "[...]",
        _ => "[ ! ]",
    };
    let reason = v["reason"].as_str().map(|r| format!(" ({r})")).unwrap_or_default();
    println!("{icon} Agent access: {}{reason}", st.to_uppercase());
    match st {
        "disabled" | "blocked" => println!("      Type `enable` and press Enter to turn agent access on."),
        "enabled" => println!("      Agents may work now. Type `disable` (or quit, or close this window) to turn it off."),
        _ => {}
    }
    prompt();
}

/// Show an input prompt when a person is typing (not when commands are piped in).
fn prompt() {
    if std::io::stdin().is_terminal() {
        print!("> ");
        let _ = std::io::stdout().flush();
    }
}

pub fn run() -> std::io::Result<()> {
    let path = paths::control_socket();
    let stream = crate::transport::connect(&path).map_err(|e| std::io::Error::new(e.kind(), format!("{path}: {e}; start the broker with `w3bs desktop daemon`")))?;
    let mut writer = stream.try_clone_box()?;
    let reader = BufReader::new(stream);
    let (resp_tx, resp_rx) = mpsc::channel::<Response>();
    let last_code = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let code_slot = last_code.clone();
    // The drive request the person was last asked about: `allow`, `deny`, `postpone`, `always` answer it.
    let last_drive = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let drive_slot = last_drive.clone();
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v.get("event").and_then(Value::as_str) {
                Some("state") => print_state(&v["data"]),
                Some("enable_requested") => {
                    let code = v["data"]["code"].as_str().unwrap_or_default().to_string();
                    println!("\n  ENABLE REQUESTED. Confirmation code: {code}");
                    println!("  Type `confirm` here, or enter the code in the terminal that asked. Ignore it to refuse.");
                    *code_slot.lock().unwrap() = Some(code);
                }
                Some("approval_requested") => {
                    let a = &v["data"];
                    println!("\n  APPROVAL NEEDED [{}] agent {} wants: {} (target {})", a["id"].as_str().unwrap_or(""), a["agent"].as_str().unwrap_or(""), a["summary"].as_str().unwrap_or(""), a["target"].as_str().unwrap_or(""));
                    println!("  Agent input is paused. `approve {0}` or `deny {0}`.", a["id"].as_str().unwrap_or(""));
                }
                Some("drive_requested") => {
                    let d = &v["data"];
                    let id = d["id"].as_str().unwrap_or_default().to_string();
                    println!("\n  AGENT WANTS TO DRIVE [{id}] {} for: {}", d["agent_name"].as_str().unwrap_or(""), d["description"].as_str().unwrap_or(""));
                    println!("  Type `allow`, `deny`, `postpone` (10 min, or `postpone 30`), or `always` (allow every agent until `always off`).");
                    *drive_slot.lock().unwrap() = Some(id);
                }
                Some("drive_decided") => {
                    let mut slot = drive_slot.lock().unwrap();
                    if slot.as_deref() == v["data"]["id"].as_str() {
                        *slot = None;
                    }
                    println!("  Drive request {} is {}", v["data"]["id"].as_str().unwrap_or(""), v["data"]["status"].as_str().unwrap_or(""));
                }
                Some("drive_always_allow") => println!("  Always allow is {}", if v["data"]["on"].as_bool().unwrap_or(false) { "ON: agents drive without asking" } else { "off: you are asked first" }),
                Some("lease") if !v["data"].is_null() => println!("  Agent {} took the input lease for task {}", v["data"]["agent"].as_str().unwrap_or(""), v["data"]["task"].as_str().unwrap_or("")),
                Some("lease") => println!("  Input lease released"),
                Some(other) => println!("  event: {other} {}", v["data"]),
                None => {
                    if let Ok(r) = serde_json::from_value::<Response>(v) {
                        let _ = resp_tx.send(r);
                    }
                }
            }
        }
        println!("broker connection closed");
        std::process::exit(1);
    });
    let mut next = 0u64;
    let mut call = |method: &str, params: Value| -> Result<Value, String> {
        next += 1;
        let req = Request { v: SCHEMA_VERSION, id: next.to_string(), method: method.into(), params };
        writeln!(writer, "{}", serde_json::to_string(&req).unwrap()).and_then(|_| writer.flush()).map_err(|e| e.to_string())?;
        loop {
            let r = resp_rx.recv().map_err(|e| e.to_string())?;
            if r.id == next.to_string() {
                return match r.error {
                    Some(e) => Err(e.to_string()),
                    None => Ok(r.result.unwrap_or(Value::Null)),
                };
            }
        }
    };
    call("subscribe", json!({"native_control": true})).map_err(std::io::Error::other)?;
    println!("Agentic Desktop Layer control. {HELP}");
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next();
        let r = match cmd {
            "" => continue,
            "enable" => {
                println!("  Enabling. If your desktop shows a permission dialog (Remote Control, screen sharing), allow it there.");
                call("enable.request", json!({})).and_then(|c| match c["code"].as_str() {
                Some(code) => call("enable.confirm", json!({"code": code})).map(|_| json!("enabled")),
                None => Ok(c),
            })
            }
            "confirm" => match last_code.lock().unwrap().take() {
                Some(code) => call("enable.confirm", json!({"code": code})),
                None => Err("no pending enable request".into()),
            },
            "disable" => call("disable", json!({"reason": "disabled from the terminal control"})),
            "stop" => call("stop", json!({"reason": "stopped from the terminal control"})),
            "approve" => match arg {
                Some(id) => call("approvals.decide", json!({"approval": id, "approve": true})),
                None => Err("usage: approve <approval id>".into()),
            },
            // `deny <appr_...>` refuses an input approval; bare `deny` or `deny <drive_...>` a drive request.
            "deny" if arg.is_some_and(|a| a.starts_with("appr")) => call("approvals.decide", json!({"approval": arg.unwrap(), "approve": false})),
            "always" if arg == Some("off") => call("drive.always_allow", json!({"on": false})),
            "allow" | "deny" | "postpone" | "always" => {
                // An id, or the latest prompt; `postpone 30` is minutes, not an id.
                let minutes = if cmd == "postpone" { arg.and_then(|a| a.parse::<u64>().ok()) } else { None };
                let id = arg.filter(|a| a.starts_with("drive")).map(String::from).or_else(|| last_drive.lock().unwrap().clone());
                match id {
                    Some(id) => {
                        let decision = match cmd { "allow" => "allow", "deny" => "deny", "postpone" => "postpone", _ => "always_allow" };
                        let mut p = json!({"drive": id, "decision": decision});
                        if let Some(m) = minutes {
                            p["postpone_ms"] = json!(m.clamp(1, 240) * 60_000);
                        }
                        call("drive.decide", p)
                    }
                    None => Err("no agent is waiting to drive (`always off` turns prompting back on)".into()),
                }
            }
            "approvals" => call("approvals.list", json!({})),
            "tasks" => call("tasks.list", json!({})),
            "status" => call("status", json!({})),
            // The control app in the browser, signed in with a one-time link from this control.
            "open" => call("control.login", json!({"via": "terminal control"})).map(|v| {
                let url = v["url"].as_str().unwrap_or_default().to_string();
                match crate::control_web::open_url(&url) {
                    Ok(()) => json!(format!("Opening the control app: {url}")),
                    Err(e) => json!(format!("{e}; open this one-time link within {} s: {url}", v["expires_in_s"])),
                }
            }),
            "quit" | "exit" => break,
            _ => Err(HELP.into()),
        };
        match r {
            Ok(v) if v.is_string() => println!("  {}", v.as_str().unwrap()),
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => println!("  error: {e}"),
        }
        prompt();
    }
    let _ = call("disable", json!({"reason": "terminal control closed"}));
    Ok(())
}
