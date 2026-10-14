//! `w3bs desktop tray`: the native control as a StatusNotifierItem (KDE Plasma natively; GNOME
//! with the AppIndicator extension; Vela's shell hosts SNI too).
//!
//! It holds the native-control subscription. If the tray exits, crashes, or the desktop's tray
//! host goes away, the broker sees the control vanish and disables access (PRD section 4).
//! State is shown as text in the title and tooltip, never by icon colour alone.
//!
//! Its menu mirrors the control app (`/control`): "Open control panel…" signs the app in with a
//! one-time link, the Agents submenu lists each agent with its mode and a Revoke item, and Recent
//! activity shows the last audit entries (metadata only).

use crate::paths;
use adl_schema::rpc::{Request, Response};
use adl_schema::SCHEMA_VERSION;
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use serde_json::{json, Value};
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
    OpenPanel,
    Revoke(String, String),
    Shortcut(String),
    Quit,
}

/// The emergency shortcut disables access but keeps the tray, so the human can see the result.
impl From<crate::shortcut::Emergency> for Act {
    fn from(_: crate::shortcut::Emergency) -> Self {
        Act::Emergency
    }
}

/// (id, name, mode label, capabilities) of an active agent.
type AgentRow = (String, String, String, String);

#[derive(Default)]
struct View {
    state: String,
    reason: Option<String>,
    lease: Option<(String, String)>,
    approvals: Vec<(String, String)>,
    pending_code: Option<String>,
    note: Option<String>,
    shortcut: Option<String>,
    agents: Vec<AgentRow>,
    /// Last audit entries, newest first, already formatted
    activity: Vec<String>,
}

struct DesktopTray {
    view: View,
    tx: mpsc::Sender<Act>,
}

fn item(label: impl Into<String>, icon: &str, tx: &mpsc::Sender<Act>, act: Act) -> MenuItem<DesktopTray> {
    let tx = tx.clone();
    StandardItem { label: label.into(), icon_name: icon.into(), activate: Box::new(move |_| drop(tx.send(act.clone()))), ..Default::default() }.into()
}

fn label(text: impl Into<String>) -> MenuItem<DesktopTray> {
    StandardItem { label: text.into(), enabled: false, ..Default::default() }.into()
}

/// dbusmenu treats `_` as a mnemonic marker; show names literally.
fn lit(s: &str) -> String {
    s.replace('_', "__")
}

fn submenu(text: impl Into<String>, icon: &str, items: Vec<MenuItem<DesktopTray>>) -> MenuItem<DesktopTray> {
    SubMenu { label: text.into(), icon_name: icon.into(), submenu: items, ..Default::default() }.into()
}

/// The preset an agent's policy matches, as `w3bs desktop agents add --preset` names them.
pub(crate) fn mode_label(policy: &Value) -> String {
    let caps: Vec<&str> = policy["capabilities"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if caps.is_empty() {
        return "no access".into();
    }
    if policy["mode"] == "restricted" {
        let apps: Vec<&str> = policy["apps"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        return format!("restricted: {}", if apps.is_empty() { "no apps".into() } else { apps.join(", ") });
    }
    if caps.contains(&"pointer") || caps.contains(&"keyboard") {
        "broad desktop".into()
    } else {
        "observe".into()
    }
}

/// Local wall-clock HH:MM:SS of a millisecond timestamp.
pub(crate) fn clock(ms: u64) -> String {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes into the provided struct.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// One read-only activity line from an audit record (metadata only, as the audit stores it).
fn activity_line(r: &Value, names: &std::collections::HashMap<String, String>) -> String {
    let who = r["agent"].as_str().map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()));
    let mut s = format!("{}  {}", clock(r["ts_ms"].as_u64().unwrap_or(0)), r["op"].as_str().unwrap_or("?"));
    if let Some(w) = who {
        s += &format!(" · {w}");
    }
    s += &format!(" · {}", r["decision"].as_str().unwrap_or(""));
    if let Some(reason) = r["meta"]["reason"].as_str() {
        s += &format!(" ({reason})");
    }
    s
}

impl DesktopTray {
    fn state_text(&self) -> String {
        let s = match self.view.state.as_str() {
            "" => "Connecting",
            "enabled" => "Enabled",
            "disabled" => "Disabled",
            "enabling" => "Enabling",
            "stopping" => "Stopping",
            "blocked" => "Blocked",
            other => other,
        };
        match &self.view.reason {
            Some(r) => format!("{s} ({r})"),
            None => s.to_string(),
        }
    }
    fn enabled(&self) -> bool {
        self.view.state == "enabled"
    }
}

impl ksni::Tray for DesktopTray {
    fn id(&self) -> String {
        "w3bs-desktop".into()
    }
    fn title(&self) -> String {
        format!("Agentic Desktop Layer: {}", self.state_text())
    }
    fn category(&self) -> ksni::Category {
        ksni::Category::SystemServices
    }
    fn status(&self) -> ksni::Status {
        if self.view.lease.is_some() || !self.view.approvals.is_empty() || self.view.pending_code.is_some() {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }
    fn icon_name(&self) -> String {
        if self.enabled() { "input-mouse" } else { "changes-prevent" }.into()
    }
    fn attention_icon_name(&self) -> String {
        "dialog-warning".into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let mut d = format!("Agent access: {}", self.state_text());
        if let Some((agent, task)) = &self.view.lease {
            d += &format!("\nAgent {agent} is controlling input (task {task})");
        }
        if !self.view.approvals.is_empty() {
            d += &format!("\n{} approval(s) waiting; agent input is paused", self.view.approvals.len());
        }
        ksni::ToolTip { title: "Agentic Desktop Layer".into(), description: d, ..Default::default() }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let tx = &self.tx;
        let mut m = vec![item("Open control panel…", "preferences-system", tx, Act::OpenPanel), MenuItem::Separator, label(format!("Status: {}", self.state_text()))];
        if let Some((agent, task)) = &self.view.lease {
            m.push(label(format!("Active: {agent}, task {task}")));
        }
        if let Some(note) = &self.view.note {
            m.push(label(note.clone()));
        }
        m.push(MenuItem::Separator);
        if self.enabled() {
            m.push(item("Disable agent access", "media-playback-stop", tx, Act::Disable));
        } else {
            m.push(item("Enable agent access", "media-playback-start", tx, Act::Enable));
        }
        if let Some(code) = &self.view.pending_code {
            m.push(item(format!("Confirm the enable request from a terminal (code {code})"), "dialog-ok", tx, Act::ConfirmPending));
        }
        if !self.view.approvals.is_empty() {
            m.push(MenuItem::Separator);
            for (id, summary) in &self.view.approvals {
                m.push(label(format!("Approval: {summary}")));
                m.push(item("    Approve", "dialog-ok", tx, Act::Decide(id.clone(), true)));
                m.push(item("    Deny", "dialog-cancel", tx, Act::Decide(id.clone(), false)));
            }
        }
        m.push(MenuItem::Separator);
        let agents: Vec<MenuItem<Self>> = if self.view.agents.is_empty() {
            vec![label("No agents registered")]
        } else {
            self.view
                .agents
                .iter()
                .map(|(id, name, mode, caps)| {
                    submenu(format!("{} ({})", lit(name), lit(mode)), "", vec![label(format!("Can use: {}", lit(caps))), item(format!("Revoke {}", lit(name)), "edit-delete", tx, Act::Revoke(id.clone(), name.clone()))])
                })
                .collect()
        };
        m.push(submenu("Agents", "system-users", agents));
        let activity: Vec<MenuItem<Self>> = if self.view.activity.is_empty() { vec![label("No activity yet")] } else { self.view.activity.iter().map(|l| label(lit(l))).collect() };
        m.push(submenu("Recent activity", "document-open-recent", activity));
        m.push(MenuItem::Separator);
        m.push(item("Stop all agent activity", "process-stop", tx, Act::Stop));
        m.push(item("Quit and disable", "application-exit", tx, Act::Quit));
        if let Some(s) = &self.view.shortcut {
            m.push(label(format!("Emergency stop: {s}")));
        }
        m
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        // A left click toggles off, never on: enabling always goes through the menu.
        if self.enabled() {
            let _ = self.tx.send(Act::Disable);
        }
    }
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        // The tray host went away: the human can no longer see or reach the switch.
        let _ = self.tx.send(Act::Quit);
        false
    }
}

/// Agent id -> registered name, over a second control connection (the subscribed one is the
/// event stream). Falls back to the id.
fn agent_name(id: &str) -> String {
    let lookup = || -> Option<String> {
        let mut c = crate::client::Client::connect(&paths::control_socket()).ok()?;
        let v = c.call("agents.list", json!({})).ok()?;
        v.as_array()?.iter().find(|a| a["id"] == id)?["name"].as_str().map(Into::into)
    };
    lookup().unwrap_or_else(|| id.to_string())
}

pub fn run() -> std::io::Result<()> {
    let path = paths::control_socket();
    let stream = crate::transport::connect(&path).map_err(|e| std::io::Error::new(e.kind(), format!("{path}: {e}; start the broker with `w3bs desktop daemon`")))?;
    let mut writer = stream.try_clone_box()?;
    let reader = BufReader::new(stream);
    let (act_tx, act_rx) = mpsc::channel::<Act>();
    let (resp_tx, resp_rx) = mpsc::channel::<Response>();
    let (ev_tx, ev_rx) = mpsc::channel::<Value>();

    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("event").is_some() {
                let _ = ev_tx.send(v);
            } else if let Ok(r) = serde_json::from_value::<Response>(v) {
                let _ = resp_tx.send(r);
            }
        }
        eprintln!("w3bs-desktop tray: broker connection closed");
        std::process::exit(1);
    });

    let tray = DesktopTray { view: View::default(), tx: act_tx.clone() };
    let handle = tray.spawn().map_err(|e| std::io::Error::other(format!("no StatusNotifierItem host ({e}); on GNOME enable the AppIndicator extension, or use `w3bs desktop control`")))?;

    // Events update the view.
    {
        let h = handle.clone();
        std::thread::spawn(move || {
            for v in ev_rx {
                let data = &v["data"];
                // Events carry agent ids; the human approves by name.
                let who = data["agent"].as_str().map(agent_name).unwrap_or_default();
                h.update(|t| match v["event"].as_str() {
                    Some("state") => {
                        t.view.state = data["state"].as_str().unwrap_or("").into();
                        t.view.reason = data["reason"].as_str().map(Into::into);
                        if t.view.state != "enabling" {
                            t.view.pending_code = None;
                        }
                        if t.view.state != "enabled" {
                            t.view.lease = None;
                            t.view.approvals.clear();
                        }
                    }
                    Some("enable_requested") => t.view.pending_code = data["code"].as_str().map(Into::into),
                    Some("lease") => t.view.lease = data.as_object().map(|l| (who.clone(), l["task"].as_str().unwrap_or("").into())),
                    Some("approval_requested") => t.view.approvals.push((data["id"].as_str().unwrap_or("").into(), format!("{who} wants: {}", data["summary"].as_str().unwrap_or("")))),
                    Some("approval_decided") => t.view.approvals.retain(|(id, _)| Some(id.as_str()) != data["id"].as_str()),
                    _ => {}
                });
            }
        });
    }

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
    call("subscribe", json!({"native_control": true})).map_err(std::io::Error::other)?;

    // Agents and recent activity change without broker events; refresh them on a second
    // connection every few seconds (and right after this tray changes something).
    let (poke_tx, poke_rx) = mpsc::channel::<()>();
    {
        let h = handle.clone();
        std::thread::spawn(move || loop {
            let snapshot = (|| -> Option<(Vec<AgentRow>, Vec<String>)> {
                let mut c = crate::client::Client::connect(&paths::control_socket()).ok()?;
                let list = c.call("agents.list", json!({})).ok()?;
                let recent = c.call("audit.recent", json!({"limit": 5})).ok()?;
                let all = list.as_array().cloned().unwrap_or_default();
                let names: std::collections::HashMap<String, String> = all.iter().map(|a| (a["id"].as_str().unwrap_or("").to_string(), a["name"].as_str().unwrap_or("").to_string())).collect();
                let agents = all
                    .iter()
                    .filter(|a| a["revoked"] != true)
                    .map(|a| {
                        let caps = a["policy"]["capabilities"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()).unwrap_or_else(|| "nothing".into());
                        (a["id"].as_str().unwrap_or("").to_string(), a["name"].as_str().unwrap_or("").to_string(), mode_label(&a["policy"]), caps)
                    })
                    .collect();
                let activity = recent.as_array().map(|r| r.iter().rev().map(|e| activity_line(e, &names)).collect()).unwrap_or_default();
                Some((agents, activity))
            })();
            if let Some((agents, activity)) = snapshot {
                h.update(|t| {
                    if t.view.agents != agents || t.view.activity != activity {
                        t.view.agents = agents;
                        t.view.activity = activity;
                    }
                });
            }
            let _ = poke_rx.recv_timeout(std::time::Duration::from_secs(3));
        });
    }

    // Off the action loop: binding can wait on the desktop's confirmation dialog (KDE shows one),
    // and a menu click made meanwhile must act now, not minutes later when the dialog closes.
    {
        let (h, tx) = (handle.clone(), act_tx.clone());
        std::thread::spawn(move || match crate::shortcut::register(tx.clone()) {
            Ok(desc) => {
                h.update(|t| t.view.shortcut = Some(desc.clone()));
                let _ = tx.send(Act::Shortcut(desc));
            }
            Err(e) => eprintln!("w3bs-desktop tray: emergency-stop shortcut unavailable ({e}); use the tray menu"),
        });
    }

    for act in act_rx {
        let r = match &act {
            Act::Enable => call("enable.request", json!({})).and_then(|c| match c["code"].as_str() {
                Some(code) => call("enable.confirm", json!({"code": code})),
                None => Err("the broker did not hand the code to this control".into()),
            }),
            Act::ConfirmPending => match handle.update(|t| t.view.pending_code.take()).flatten() {
                Some(code) => call("enable.confirm", json!({"code": code})),
                None => Err("no pending enable request".into()),
            },
            Act::Disable => call("disable", json!({"reason": "disabled from the tray"})),
            Act::Stop => call("stop", json!({"reason": "stopped from the tray"})),
            // The broker's own emergency-stop event: disables, and is audited as such.
            Act::Emergency => call("session.event", json!({"event": "emergency_stop"})),
            Act::Decide(id, ok) => call("approvals.decide", json!({"approval": id, "approve": ok})),
            // A one-time sign-in link minted on this (the native control's) connection.
            Act::OpenPanel => call("control.login", json!({"via": "tray"})).and_then(|v| {
                let url = v["url"].as_str().unwrap_or_default();
                crate::control_web::open_url(url).map(|_| Value::Null).map_err(|e| format!("{e}; run `w3bs desktop control --open`"))
            }),
            Act::Revoke(id, _name) => call("agents.revoke", json!({"agent": id})),
            // Tell the broker which shortcut the desktop bound, so the control app can show it.
            Act::Shortcut(desc) => call("native.shortcut", json!({"description": desc})),
            Act::Quit => {
                let _ = call("disable", json!({"reason": "tray closed"}));
                handle.shutdown().wait();
                return Ok(());
            }
        };
        let note = r.err().map(|e| format!("Last action failed: {e}"));
        handle.update(|t| t.view.note = note);
        let _ = poke_tx.send(());
    }
    Ok(())
}
