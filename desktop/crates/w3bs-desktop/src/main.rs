//! `w3bs desktop`: Agentic Desktop Layer CLI, broker daemon, terminal control and MCP server.

mod client;
mod control;
mod mcp;
mod paths;
mod peer;
mod server;
mod shortcut;
mod tray;

use adl_broker::audit::Audit;
use adl_broker::store::AgentStore;
use adl_broker::{Adapter, Broker, Config};
use adl_schema::*;
use base64::Engine as _;
use clap::{Args, Parser, Subcommand, ValueEnum};
use client::Client;
use serde_json::{json, Value};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

const AFTER_HELP: &str = "\
Agent access starts Disabled. A human enables it from the native control (tray, panel extension
or `w3bs desktop control`); agents cannot enable themselves or approve their own requests.

Exit codes: 0 ok, 1 operation failed or outcome unknown, 2 invalid request, 3 human approval
required, 4 disabled or blocked, 5 busy (another agent holds the input lease).

Docs: https://w3bs.org/desktop";

#[derive(Parser)]
#[command(name = "w3bs desktop", bin_name = "w3bs desktop", version, about = "Agentic Desktop Layer: let authorized agents operate your real desktop, behind your switch", after_help = AFTER_HELP)]
struct Cli {
    /// Print the raw JSON result
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum AdapterKind {
    /// Vela if running, else the XDG portal backend (KDE Plasma, GNOME; plus KWin windows on Plasma); never the mock
    Auto,
    /// Vela compositor command socket (observation and interlocks; input needs VELA_DEBUG_INPUT)
    Vela,
    /// XDG RemoteDesktop/ScreenCast/Screenshot portals (KDE Plasma, GNOME); on Plasma windows come from KWin
    Portal,
    /// Deterministic fake desktop for tests and demos
    Mock,
}

fn make_adapter(kind: AdapterKind) -> Result<Arc<dyn Adapter>> {
    use adl_linux::kwin::{KWinObserver, KWinScripts, WithKWin};
    use adl_linux::{portal::PortalAdapter, vela::VelaAdapter};
    let vela = || VelaAdapter::default_socket().map(VelaAdapter::new).ok_or_else(|| AdlError::new(ErrorCode::Blocked, "Vela command socket not found (needs XDG_RUNTIME_DIR and WAYLAND_DISPLAY)"));
    // On Plasma the portal backend gets its window list, focus and stacking from KWin.
    let portal = || -> Arc<dyn Adapter> {
        let p = PortalAdapter::new(paths::config_dir().join("portal-restore-token"));
        if !KWinObserver::detect() {
            return Arc::new(p);
        }
        match KWinScripts::connect() {
            Ok(src) => Arc::new(WithKWin::new(p, KWinObserver::new(Box::new(src)))),
            Err(e) => {
                eprintln!("w3bs-desktop: KWin window observer unavailable ({}); targeted input will be refused", e.message);
                Arc::new(p)
            }
        }
    };
    Ok(match kind {
        AdapterKind::Mock => Arc::new(adl_broker::mock::MockDesktop::new()),
        AdapterKind::Vela => Arc::new(vela()?),
        AdapterKind::Portal => portal(),
        AdapterKind::Auto if VelaAdapter::detect() => Arc::new(vela()?),
        AdapterKind::Auto if PortalAdapter::detect() => portal(),
        AdapterKind::Auto => return Err(AdlError::new(ErrorCode::Blocked, "no supported desktop session found (need a Wayland/X11 session with D-Bus, or Vela); use --adapter mock for a demo")),
    })
}

#[derive(Args, Clone)]
struct AgentAuth {
    /// Registered agent name or id whose saved token to use (default: $W3BS_DESKTOP_TOKEN)
    #[arg(long, env = "W3BS_DESKTOP_AGENT")]
    agent: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the local broker (normally started by the user service)
    Daemon {
        #[arg(long, value_enum, default_value = "auto")]
        adapter: AdapterKind,
        /// Allow enabling with no native control attached (headless test rigs only)
        #[arg(long, hide = true)]
        no_native_control: bool,
        /// Audit metadata retention in days
        #[arg(long, default_value_t = 7)]
        retention_days: u64,
    },
    /// Terminal native control: shows state, enable codes and approvals; quitting disables access
    Control,
    /// Tray/panel native control (StatusNotifierItem) with an emergency-stop shortcut; quitting disables access
    Tray,
    /// Show the layer state
    Status,
    /// Ask to enable agent access (confirm in the native control or with its code)
    Enable,
    /// Disable agent access and stop everything mediated by the layer
    Disable,
    /// Stop all agent activity (cancel tasks, revoke the lease) but stay enabled
    Stop {
        #[arg(long)]
        all: bool,
    },
    /// Diagnose the session, backend and permissions
    Doctor,
    /// What this desktop backend can do
    Capabilities,
    /// Register and manage agents
    #[command(subcommand)]
    Agents(AgentsCmd),
    /// Human approvals for requests that need them
    #[command(subcommand)]
    Approvals(ApprovalsCmd),
    /// Inspect and cancel tasks
    #[command(subcommand)]
    Tasks(TasksCmd),
    /// Local audit metadata (no screenshots, text or message bodies are kept)
    #[command(subcommand)]
    Audit(AuditCmd),
    /// Displays, their logical geometry and the layout generation (agent)
    Layout {
        #[command(flatten)]
        auth: AgentAuth,
    },
    /// List windows (agent)
    #[command(subcommand)]
    Windows(WindowsCmd),
    /// Accessibility tree of a window (agent)
    Inspect {
        #[arg(long)]
        window: String,
        #[command(flatten)]
        auth: AgentAuth,
    },
    /// Screen capture (agent)
    #[command(subcommand)]
    Screen(ScreenCmd),
    /// Pointer control (agent; acquires the input lease for the duration of the command)
    #[command(subcommand)]
    Mouse(MouseCmd),
    /// Keyboard control (agent; acquires the input lease for the duration of the command)
    #[command(subcommand)]
    Keyboard(KeyboardCmd),
    /// Agent transport
    #[command(subcommand)]
    Mcp(McpCmd),
    /// Print the MCP setup for an agent client
    Setup {
        #[arg(value_enum)]
        client: SetupClient,
        /// Registered agent name whose token file to use
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SetupClient {
    ClaudeCode,
    Opencode,
    Codex,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Preset {
    /// Windows, accessibility and screenshots only; no input
    Observe,
    /// Pointer and keyboard confined to the --app allowlist
    Restricted,
    /// Whole-desktop observation and input (business-intent limits cannot be enforced)
    Broad,
    /// Nothing until updated
    None,
}

#[derive(Subcommand)]
enum AgentsCmd {
    /// Register an agent and save its token (0600) for MCP setup
    Add {
        name: String,
        #[arg(long, value_enum, default_value = "observe")]
        preset: Preset,
        /// App id (or prefix*) the agent may act on in restricted mode; repeatable
        #[arg(long = "app")]
        apps: Vec<String>,
        /// Action kinds or `input` that need per-request approval, e.g. type, shortcut
        #[arg(long = "approve-for")]
        approve_for: Vec<String>,
        /// Grant expiry in hours
        #[arg(long)]
        hours: Option<u64>,
        /// Print the token instead of saving it
        #[arg(long)]
        print_token: bool,
    },
    List,
    /// Replace an agent's policy with a preset or a JSON policy file
    Update {
        agent: String,
        #[arg(long)]
        policy: String,
        #[arg(long = "app")]
        apps: Vec<String>,
    },
    Revoke { agent: String },
    Remove { agent: String },
}

#[derive(Subcommand)]
enum ApprovalsCmd {
    List,
    Approve { id: String },
    Deny { id: String },
}

#[derive(Subcommand)]
enum TasksCmd {
    List,
    Inspect { id: String },
    Cancel { id: String },
}

#[derive(Subcommand)]
enum AuditCmd {
    Export,
    Delete,
}

#[derive(Subcommand)]
enum WindowsCmd {
    List {
        #[command(flatten)]
        auth: AgentAuth,
    },
}

#[derive(Subcommand)]
enum ScreenCmd {
    Capture {
        #[arg(long)]
        display: String,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        auth: AgentAuth,
    },
}

#[derive(Args, Clone)]
struct Where {
    #[arg(long, allow_hyphen_values = true)]
    x: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    y: Option<f64>,
    /// Coordinates are relative to this display
    #[arg(long, conflicts_with_all = ["window", "capture"])]
    display: Option<String>,
    /// Coordinates are relative to this window, which must be the one under the pointer
    #[arg(long, conflicts_with = "capture")]
    window: Option<String>,
    /// Coordinates are pixels of this screenshot
    #[arg(long)]
    capture: Option<String>,
}

impl Where {
    fn space(&self) -> Value {
        match (&self.display, &self.window, &self.capture) {
            (Some(d), _, _) => json!({"space": "display", "display": d}),
            (_, Some(w), _) => json!({"space": "window", "window": w}),
            (_, _, Some(c)) => json!({"space": "capture", "capture": c}),
            _ => json!({"space": "desktop"}),
        }
    }
    fn at(&self, x: f64, y: f64) -> Value {
        let mut p = self.space();
        p["x"] = json!(x);
        p["y"] = json!(y);
        p
    }
    fn point(&self) -> Option<Value> {
        Some(self.at(self.x?, self.y?))
    }
}

fn pair(s: &str) -> Result<(f64, f64), String> {
    let (a, b) = s.split_once(',').ok_or("expected X,Y")?;
    Ok((a.trim().parse().map_err(|_| "bad X")?, b.trim().parse().map_err(|_| "bad Y")?))
}

#[derive(Subcommand)]
enum MouseCmd {
    Move {
        #[command(flatten)]
        at: Where,
        #[command(flatten)]
        auth: AgentAuth,
    },
    Click {
        #[arg(long, default_value = "left")]
        button: String,
        #[arg(long, default_value_t = 1)]
        count: u8,
        #[command(flatten)]
        at: Where,
        #[command(flatten)]
        auth: AgentAuth,
    },
    Drag {
        #[arg(long, value_parser = pair, allow_hyphen_values = true)]
        from: (f64, f64),
        #[arg(long, value_parser = pair, allow_hyphen_values = true)]
        to: (f64, f64),
        #[arg(long, default_value_t = 20)]
        steps: u32,
        #[command(flatten)]
        at: Where,
        #[command(flatten)]
        auth: AgentAuth,
    },
    Scroll {
        #[arg(long, default_value = "vertical")]
        axis: String,
        #[arg(long, allow_hyphen_values = true)]
        steps: i32,
        #[command(flatten)]
        at: Where,
        #[command(flatten)]
        auth: AgentAuth,
    },
}

#[derive(Subcommand)]
enum KeyboardCmd {
    /// Type text from a file or stdin (never from arguments, to keep it out of shell history)
    Type {
        #[arg(long)]
        window: Option<String>,
        #[arg(long, conflicts_with = "stdin")]
        from_file: Option<PathBuf>,
        #[arg(long)]
        stdin: bool,
        #[command(flatten)]
        auth: AgentAuth,
    },
    Press {
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        key: String,
        #[command(flatten)]
        auth: AgentAuth,
    },
    Shortcut {
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        keys: String,
        #[command(flatten)]
        auth: AgentAuth,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Serve MCP over stdio for one agent
    Serve {
        #[arg(long)]
        stdio: bool,
        #[command(flatten)]
        auth: AgentAuth,
    },
}

fn exit_code(e: &AdlError) -> i32 {
    match e.code {
        ErrorCode::Invalid | ErrorCode::NotFound => 2,
        ErrorCode::ApprovalRequired => 3,
        ErrorCode::Disabled | ErrorCode::Blocked | ErrorCode::Unauthenticated | ErrorCode::Denied => 4,
        ErrorCode::Busy => 5,
        _ => 1,
    }
}

fn fail(e: AdlError, json_out: bool) -> ! {
    if json_out {
        println!("{}", json!({"error": e}));
    } else {
        eprintln!("error: {e}");
        if !e.details.is_null() {
            eprintln!("       {}", e.details);
        }
    }
    std::process::exit(exit_code(&e));
}

fn control_client() -> Result<Client> {
    Client::connect(&paths::control_socket())
}

fn token_for(auth: &AgentAuth) -> Result<String> {
    if let Some(name) = &auth.agent {
        let path = paths::token_file(name);
        return std::fs::read_to_string(&path).map(|s| s.trim().to_string()).map_err(|e| AdlError::new(ErrorCode::Unauthenticated, format!("{}: {e}; register with `w3bs desktop agents add {name}`", path.display())));
    }
    std::env::var("W3BS_DESKTOP_TOKEN").map_err(|_| AdlError::new(ErrorCode::Unauthenticated, "pass --agent NAME or set W3BS_DESKTOP_TOKEN"))
}

fn agent_client(auth: &AgentAuth) -> Result<Client> {
    let token = token_for(auth)?;
    let mut c = Client::connect(&paths::agent_socket())?;
    c.call("hello", json!({"token": token, "client": "cli"}))?;
    Ok(c)
}

/// One CLI input command = begin task, take lease, run one batch, end task.
fn run_input(auth: &AgentAuth, what: &str, window: Option<String>, actions: Vec<Value>) -> Result<Value> {
    let mut c = agent_client(auth)?;
    let task = c.call("task.begin", json!({"description": format!("cli: {what}")}))?;
    let id = task["id"].as_str().unwrap_or_default().to_string();
    let result = c.call("lease.acquire", json!({"task": id})).and_then(|_| c.call("input", json!({"task": id, "target": {"window": window}, "actions": actions})));
    let outcome = match &result {
        Ok(r) if r["outcome"] == "completed" => "completed",
        Ok(_) => "failed",
        Err(_) => "cancelled",
    };
    let _ = c.call("task.end", json!({"task": id, "outcome": outcome}));
    let r = result?;
    if r["outcome"] != "completed" {
        let e: AdlError = serde_json::from_value(r["error"].clone()).unwrap_or_else(|_| AdlError::internal(format!("batch {}", r["outcome"])));
        return Err(e.with_details(json!({"outcome": r["outcome"], "actions_completed": r["actions_completed"]})));
    }
    Ok(r)
}

fn preset_policy(p: Preset, apps: Vec<String>, approve_for: Vec<String>, hours: Option<u64>) -> Policy {
    use Capability::*;
    let (mode, caps): (ControlMode, &[Capability]) = match p {
        Preset::Observe => (ControlMode::BroadDesktop, &[Windows, Accessibility, ScreenCapture]),
        Preset::Restricted => (ControlMode::Restricted, &[Windows, Accessibility, Pointer, Keyboard]),
        Preset::Broad => (ControlMode::BroadDesktop, &[Windows, Accessibility, ScreenCapture, Pointer, Keyboard]),
        Preset::None => (ControlMode::Restricted, &[]),
    };
    Policy {
        capabilities: caps.iter().copied().collect(),
        mode,
        apps,
        approval_required_for: approve_for.into_iter().collect(),
        max_actions_per_minute: 0,
        expires_at_ms: hours.map(|h| adl_broker::util::now_ms() + h * 3_600_000),
    }
}

fn save_token(name: &str, token: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let path = paths::token_file(name);
    let dir = path.parent().unwrap();
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    f.write_all(token.as_bytes())?;
    Ok(path)
}

fn print(v: &Value, json_out: bool) {
    if json_out || !v.is_string() {
        println!("{}", serde_json::to_string_pretty(v).unwrap());
    } else {
        println!("{}", v.as_str().unwrap());
    }
}

fn status_line(v: &Value) -> String {
    let mut s = format!("Agent access: {}", v["state"].as_str().unwrap_or("?").to_uppercase());
    if let Some(r) = v["blocked_reason"].as_str() {
        s += &format!(" ({r})");
    }
    s += &format!("\nAdapter: {}  Agents: {}  Pending approvals: {}", v["adapter"].as_str().unwrap_or(""), v["agents"], v["pending_approvals"]);
    match v["lease"].as_object() {
        Some(l) => s += &format!("\nInput lease: agent {} task {}", l["agent"].as_str().unwrap_or(""), l["task"].as_str().unwrap_or("")),
        None => s += "\nInput lease: free",
    }
    for t in v["running_tasks"].as_array().into_iter().flatten() {
        s += &format!("\n  task {} [{}] {}", t["id"].as_str().unwrap_or(""), t["agent"].as_str().unwrap_or(""), t["description"].as_str().unwrap_or(""));
    }
    s
}

fn main() {
    let cli = Cli::parse();
    let j = cli.json;
    let r: Result<Value> = (|| match cli.cmd {
        Cmd::Daemon { adapter, no_native_control, retention_days } => {
            let adapter = make_adapter(adapter)?;
            let store = AgentStore::open(paths::config_dir().join("agents.json"))?;
            let audit = Audit::open(paths::state_dir().join("audit"), retention_days);
            let cfg = Config { require_native_control: !no_native_control, ..Config::default() };
            let broker = Arc::new(Broker::new(adapter, store, audit, cfg));
            server::serve(broker).map_err(|e| AdlError::new(ErrorCode::Blocked, e.to_string()))?;
            Ok(Value::Null)
        }
        Cmd::Tray => {
            tray::run().map_err(|e| AdlError::new(ErrorCode::Blocked, e.to_string()))?;
            Ok(Value::Null)
        }
        Cmd::Control => {
            control::run().map_err(|e| AdlError::new(ErrorCode::Blocked, e.to_string()))?;
            Ok(Value::Null)
        }
        Cmd::Status => {
            let v = control_client()?.call("status", json!({}))?;
            Ok(if j { v } else { json!(status_line(&v)) })
        }
        Cmd::Enable => {
            let mut c = control_client()?;
            let ch = c.call("enable.request", json!({}))?;
            if !std::io::stdin().is_terminal() {
                return Ok(json!({"requested": ch, "next": "confirm in the native control"}));
            }
            eprint!("Enter the code shown in your tray, panel or `w3bs desktop control` (blank to cancel): ");
            let mut code = String::new();
            std::io::stdin().read_line(&mut code).ok();
            if code.trim().is_empty() {
                return Ok(json!("not enabled"));
            }
            let s = c.call("enable.confirm", json!({"code": code.trim()}))?;
            Ok(if j { s } else { json!(status_line(&s)) })
        }
        Cmd::Disable => {
            let v = control_client()?.call("disable", json!({"reason": "disabled from the CLI"}))?;
            Ok(if j { v } else { json!(format!("Agent access: DISABLED ({} tasks cancelled, {} inputs released). {}", v["tasks_cancelled"], v["synthetic_inputs_released"].as_array().map_or(0, Vec::len), v["note"].as_str().unwrap_or(""))) })
        }
        Cmd::Stop { all: _ } => control_client()?.call("stop", json!({"reason": "stopped from the CLI"})),
        Cmd::Doctor => control_client()?.call("doctor", json!({})),
        Cmd::Capabilities => control_client()?.call("capabilities", json!({})),
        Cmd::Agents(a) => {
            let mut c = control_client()?;
            match a {
                AgentsCmd::Add { name, preset, apps, approve_for, hours, print_token } => {
                    if preset == Preset::Restricted && apps.is_empty() {
                        return Err(AdlError::invalid("restricted agents need at least one --app"));
                    }
                    if preset == Preset::Broad {
                        eprintln!("warning: broad desktop control lets {name} see and operate every application. Per-app or draft-only limits cannot be enforced in this mode.");
                    }
                    let v = c.call("agents.add", json!({"name": name, "policy": preset_policy(preset, apps, approve_for, hours)}))?;
                    let token = v["token"].as_str().unwrap_or_default();
                    if print_token {
                        return Ok(v);
                    }
                    let path = save_token(&name, token).map_err(|e| AdlError::internal(e.to_string()))?;
                    Ok(json!({"agent": v["agent"], "token_file": path, "next": format!("w3bs desktop setup claude-code --agent {name}")}))
                }
                AgentsCmd::List => c.call("agents.list", json!({})),
                AgentsCmd::Update { agent, policy, apps } => {
                    let pol = match policy.as_str() {
                        "observe" => preset_policy(Preset::Observe, apps, vec![], None),
                        "restricted" => preset_policy(Preset::Restricted, apps, vec![], None),
                        "broad" => preset_policy(Preset::Broad, apps, vec![], None),
                        "none" => preset_policy(Preset::None, apps, vec![], None),
                        file => serde_json::from_slice(&std::fs::read(file).map_err(|e| AdlError::invalid(format!("{file}: {e}")))?).map_err(|e| AdlError::invalid(format!("{file}: {e}")))?,
                    };
                    c.call("agents.update", json!({"agent": agent, "policy": pol}))
                }
                AgentsCmd::Revoke { agent } => c.call("agents.revoke", json!({"agent": agent})),
                AgentsCmd::Remove { agent } => {
                    let v = c.call("agents.remove", json!({"agent": agent}))?;
                    let _ = std::fs::remove_file(paths::token_file(&agent));
                    Ok(v)
                }
            }
        }
        Cmd::Approvals(a) => {
            let mut c = control_client()?;
            match a {
                ApprovalsCmd::List => c.call("approvals.list", json!({})),
                ApprovalsCmd::Approve { id } => c.call("approvals.decide", json!({"approval": id, "approve": true})),
                ApprovalsCmd::Deny { id } => c.call("approvals.decide", json!({"approval": id, "approve": false})),
            }
        }
        Cmd::Tasks(t) => {
            let mut c = control_client()?;
            match t {
                TasksCmd::List => c.call("tasks.list", json!({})),
                TasksCmd::Inspect { id } => c.call("tasks.inspect", json!({"task": id})),
                TasksCmd::Cancel { id } => c.call("tasks.cancel", json!({"task": id})),
            }
        }
        Cmd::Audit(a) => {
            let mut c = control_client()?;
            match a {
                AuditCmd::Export => c.call("audit.export", json!({})),
                AuditCmd::Delete => c.call("audit.delete", json!({})),
            }
        }
        Cmd::Layout { auth } => agent_client(&auth)?.call("layout", json!({})),
        Cmd::Windows(WindowsCmd::List { auth }) => agent_client(&auth)?.call("windows", json!({})),
        Cmd::Inspect { window, auth } => agent_client(&auth)?.call("inspect", json!({"window": window})),
        Cmd::Screen(ScreenCmd::Capture { display, output, auth }) => {
            let mut v = agent_client(&auth)?.call("screenshot", json!({"display": display}))?;
            let png = base64::engine::general_purpose::STANDARD.decode(v["png_base64"].as_str().unwrap_or_default()).map_err(|e| AdlError::internal(e.to_string()))?;
            std::fs::write(&output, png).map_err(|e| AdlError::internal(format!("{}: {e}", output.display())))?;
            v.as_object_mut().unwrap().remove("png_base64");
            v["output"] = json!(output);
            Ok(v)
        }
        Cmd::Mouse(m) => match m {
            MouseCmd::Move { at, auth } => {
                let to = at.point().ok_or_else(|| AdlError::invalid("--x and --y are required"))?;
                run_input(&auth, "mouse move", None, vec![json!({"type": "move", "to": to})])
            }
            MouseCmd::Click { button, count, at, auth } => {
                let mut a = json!({"type": "click", "button": button, "count": count});
                if let Some(p) = at.point() {
                    a["at"] = p;
                }
                run_input(&auth, "mouse click", at.window.clone(), vec![a])
            }
            MouseCmd::Drag { from, to, steps, at, auth } => run_input(&auth, "mouse drag", at.window.clone(), vec![json!({"type": "drag", "from": at.at(from.0, from.1), "to": at.at(to.0, to.1), "steps": steps})]),
            MouseCmd::Scroll { axis, steps, at, auth } => {
                let mut a = json!({"type": "scroll", "axis": axis, "steps": steps});
                if let Some(p) = at.point() {
                    a["at"] = p;
                }
                run_input(&auth, "mouse scroll", at.window.clone(), vec![a])
            }
        },
        Cmd::Keyboard(k) => match k {
            KeyboardCmd::Type { window, from_file, stdin, auth } => {
                let text = match (from_file, stdin) {
                    (Some(f), _) => std::fs::read_to_string(&f).map_err(|e| AdlError::invalid(format!("{}: {e}", f.display())))?,
                    (None, true) => {
                        let mut s = String::new();
                        std::io::stdin().read_to_string(&mut s).map_err(|e| AdlError::invalid(e.to_string()))?;
                        s
                    }
                    (None, false) => return Err(AdlError::invalid("pass --from-file PATH or --stdin; text is never taken from arguments")),
                };
                run_input(&auth, "keyboard type", window, vec![json!({"type": "type", "text": text})])
            }
            KeyboardCmd::Press { window, key, auth } => run_input(&auth, "keyboard press", window, vec![json!({"type": "key_press", "key": key})]),
            KeyboardCmd::Shortcut { window, keys, auth } => run_input(&auth, "keyboard shortcut", window, vec![json!({"type": "shortcut", "keys": keys})]),
        },
        Cmd::Mcp(McpCmd::Serve { stdio, auth }) => {
            if !stdio {
                return Err(AdlError::invalid("only --stdio is supported; HTTP transport is not offered"));
            }
            mcp::serve_stdio(token_for(&auth)?)?;
            Ok(Value::Null)
        }
        Cmd::Setup { client, agent } => {
            let name = agent.unwrap_or_else(|| "claude-code".into());
            if !paths::token_file(&name).exists() {
                return Err(AdlError::not_found(format!("no saved token for {name}; run `w3bs desktop agents add {name}` first")));
            }
            let bin = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "w3bs-desktop".into());
            Ok(json!(match client {
                SetupClient::ClaudeCode => format!("claude mcp add w3bs-desktop -- {bin} mcp serve --stdio --agent {name}"),
                SetupClient::Opencode => serde_json::to_string_pretty(&json!({"mcp": {"w3bs-desktop": {"type": "local", "command": [bin, "mcp", "serve", "--stdio", "--agent", name], "enabled": true}}})).unwrap(),
                SetupClient::Codex => format!("# ~/.codex/config.toml\n[mcp_servers.w3bs-desktop]\ncommand = \"{bin}\"\nargs = [\"mcp\", \"serve\", \"--stdio\", \"--agent\", \"{name}\"]"),
            }))
        }
    })();
    match r {
        Ok(Value::Null) => {}
        Ok(v) => print(&v, j),
        Err(e) => fail(e, j),
    }
}
