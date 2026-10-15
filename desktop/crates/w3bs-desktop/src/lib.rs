//! `w3bs desktop`: Agentic Desktop Layer CLI, broker daemon, terminal control and MCP server.
//!
//! The whole program lives in this library so two executables can share it: `w3bs-desktop`
//! (the console CLI) and, on Windows, `w3bs-desktopw` (the same program linked for the GUI
//! subsystem, used to run the broker and the tray at logon without a console window).

mod background;
mod client;
mod control;
mod control_web;
#[cfg(target_os = "linux")]
mod gnome_ext;
mod http_mcp;
mod mcp;
mod paths;
mod peer;
mod platform;
mod server;
#[cfg(target_os = "linux")]
mod shortcut;
mod transport;
#[cfg(target_os = "linux")]
mod tray;

use adl_broker::audit::Audit;
use adl_broker::store::AgentStore;
use adl_broker::{Broker, Config};
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
The control app (`w3bs desktop control --open`, or Open control panel in the tray) shows
agents, tasks, approvals and activity in your browser at http://127.0.0.1:7801/control.
Non-desktop instances (containers, VMs, CI) with nobody at the screen start the broker with
`w3bs desktop daemon --enabled-by-default` instead: Enabled from startup, no native control.

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
    /// Vela if running, else the XDG portal backend (KDE Plasma, GNOME; windows from KWin or the W3BS GNOME Shell extension); never the mock
    Auto,
    /// Vela compositor command socket (observation and interlocks; input needs VELA_DEBUG_INPUT)
    Vela,
    /// XDG RemoteDesktop/ScreenCast/Screenshot portals (KDE Plasma, GNOME); windows come from KWin or the GNOME Shell extension
    Portal,
    /// Windows (UI Automation + SendInput) or macOS (Accessibility + CGEvent) native backend
    Native,
    /// Deterministic fake desktop for tests and demos
    Mock,
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
        /// Non-desktop instances (containers, VMs, CI) with nobody at the screen: start with agent
        /// access Enabled, need no tray/panel/control, and let `enable` skip the confirmation code.
        /// Never use it on a person's own desktop.
        #[arg(long, env = "W3BS_DESKTOP_ENABLED_BY_DEFAULT")]
        enabled_by_default: bool,
        /// Audit metadata retention in days
        #[arg(long, default_value_t = 7)]
        retention_days: u64,
        /// Also serve MCP over HTTP on this loopback address (e.g. 127.0.0.1:7801), so agents
        /// connect to the running broker instead of spawning `mcp serve --stdio`
        #[arg(long, value_name = "ADDR", env = "W3BS_DESKTOP_MCP_HTTP")]
        mcp_http: Option<String>,
    },
    /// Terminal native control: shows state, enable codes and approvals; quitting disables access
    #[command(long_about = "Terminal native control: the on/off switch for agent access, in a terminal.\n\nRun it, then type a command and press Enter:\n  enable            turn agent access on (your desktop may ask for permission the first time)\n  disable           turn it off\n  stop              cancel all agent activity but stay enabled\n  approvals         list requests waiting for you\n  approve <id>      allow one request; deny <id> refuses it\n  confirm           confirm an enable request started from another terminal\n  tasks, status     what agents are doing\n  open              open the control app in your browser\n  quit              leave (this also turns agent access off)\n\nKeep it open while agents work. The tray icon or GNOME panel menu does the same thing.\n\nWith --open it does not start the terminal control: it opens the control app (agents, tasks,\napprovals, activity) in your browser with a one-time sign-in link and exits. Closing the app\ndoes not turn agent access off; its Quit and disable button does.")]
    Control {
        /// Open the control app in your browser (one-time sign-in link, valid 60 s) and exit
        #[arg(long)]
        open: bool,
        /// With --open: print the sign-in link instead of starting a browser
        #[arg(long, requires = "open")]
        no_browser: bool,
    },
    /// Tray/menu-bar native control (StatusNotifierItem on Linux, a menu-bar item on macOS) with an emergency-stop shortcut; quitting disables access
    Tray {
        /// Wait up to this many seconds for the broker to come up (login items start together)
        #[arg(long, value_name = "SECONDS", default_value_t = 0)]
        wait_for_broker: u64,
    },
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
    Doctor {
        /// Show the operating system's permission prompts for anything missing (macOS)
        #[arg(long)]
        prompt: bool,
    },
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
    /// Desktop Training Mode: record demonstrations of your own work for W3BS Training
    #[command(subcommand)]
    Training(TrainingCmd),
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
    /// Print the MCP setup for an agent client, or install the GNOME Shell extension
    Setup {
        #[arg(value_enum)]
        client: SetupClient,
        /// Registered agent name whose token file to use
        #[arg(long)]
        agent: Option<String>,
        /// Connect over HTTP to a broker started with `daemon --mcp-http` instead of stdio
        #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "http://127.0.0.1:7801/mcp")]
        http: Option<String>,
    },
}

#[derive(Subcommand)]
enum TrainingCmd {
    /// What the recorder is doing
    Status,
    /// Applications with open windows (ids to pass to --app)
    Apps,
    /// Observe my work: record approved applications until you finish
    Observe {
        /// Application id whose windows may be recorded (repeat); nothing else is captured
        #[arg(long = "app", required = true)]
        apps: Vec<String>,
        /// Let agents keep their input lease while recording (steps become mixed-origin)
        #[arg(long)]
        allow_agents: bool,
    },
    /// Teach this task: one deliberate demonstration with a goal
    Teach {
        /// File holding the goal (what you are about to do); sensitive text never goes in arguments
        #[arg(long)]
        goal_file: PathBuf,
        #[arg(long = "app", required = true)]
        apps: Vec<String>,
        #[arg(long)]
        allow_agents: bool,
    },
    /// Pause recording (anything may pause; resuming needs you)
    Pause,
    /// Resume a paused recording
    Resume,
    /// Mark a task boundary (observe mode starts a new task here)
    Mark,
    /// Add your own explanation from a file
    Explain {
        #[arg(long)]
        from_file: PathBuf,
    },
    /// Finish and hand the recording to W3BS Training for review
    Finish {
        #[arg(long, value_enum)]
        outcome: Option<TrainingOutcome>,
    },
    /// Throw the current recording away
    Discard,
    /// Delete recent activity: what the recorder holds, then (in a terminal) W3BS Training's copy
    Forget {
        /// How far back, e.g. 10m, 2h, 1d
        #[arg(long)]
        last: String,
    },
    /// Retry handing finished recordings to W3BS Training
    Handoff,
    /// Open the Training section of the control app (one-time sign-in link)
    Open {
        #[arg(long)]
        no_browser: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum TrainingOutcome {
    Success,
    Partial,
    Failure,
    Interrupted,
    Unknown,
}

fn parse_last(s: &str) -> Result<u64> {
    let (n, unit) = s.trim().split_at(s.trim().len().saturating_sub(1));
    let n: u64 = n.parse().map_err(|_| AdlError::invalid("--last takes a duration like 30s, 10m, 2h or 1d"))?;
    let ms = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(AdlError::invalid("--last takes a duration like 30s, 10m, 2h or 1d")),
    };
    Ok(n * ms)
}

fn training_line(v: &Value) -> String {
    let mut s = format!("Training: {}", v["state"].as_str().unwrap_or("?").to_uppercase());
    if let Some(r) = v["reason"].as_str() {
        s += &format!(" ({r})");
    }
    if let Some(r) = v["recording"].as_object() {
        s += &format!(
            "\n{} · {} frame(s), {} step(s), {} gap(s){} · apps: {}",
            if r["mode"] == "teach" { format!("Teach: {}", r["goal"].as_str().unwrap_or("")) } else { "Observe my work".into() },
            r["frames"], r["steps"], r["gaps"],
            if r["excluded_now"].is_string() { " · not capturing (outside approved apps)" } else { "" },
            r["applications"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
        );
    }
    if let Some(p) = v["pending_handoffs"].as_array().filter(|p| !p.is_empty()) {
        s += &format!("\n{} recording(s) waiting for W3BS Training: `w3bs desktop training handoff`", p.len());
    }
    s += &format!("\nAgent input leases: {}", v["agent_leases"].as_str().unwrap_or(""));
    s
}

#[derive(Clone, Copy, ValueEnum)]
enum SetupClient {
    ClaudeCode,
    Opencode,
    Codex,
    /// Install and enable the bundled GNOME Shell extension (top-bar control + window data)
    GnomeExtension,
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
    let path = paths::token_file(name);
    let dir = path.parent().unwrap();
    let mut db = std::fs::DirBuilder::new();
    db.recursive(true);
    let mut oo = std::fs::OpenOptions::new();
    oo.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        db.mode(0o700);
        oo.mode(0o600);
    }
    // Windows: %LOCALAPPDATA% is already private to the user.
    db.create(dir)?;
    let mut f = oo.open(&path)?;
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

/// Poll the control endpoint until the broker answers or `secs` pass.
fn wait_for_broker(secs: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline && transport::connect(&paths::control_socket()).is_err() {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Entry point of the console executable `w3bs-desktop`.
pub fn run() {
    run_with(Cli::parse())
}

/// Entry point of the GUI-subsystem executable `w3bs-desktopw` (Windows background launcher for
/// `daemon` and `tray`). It has no console, so when nobody handed it standard handles its
/// output goes to a log file under the state directory instead.
pub fn run_background() {
    // Redirect before parsing, so even a usage error lands in the log.
    let name = std::env::args().skip(1).find(|a| a == "daemon" || a == "tray").unwrap_or_else(|| "w3bs-desktopw".into());
    background::redirect_output(&paths::state_dir().join("logs"), &name);
    run_with(Cli::parse())
}

fn run_with(cli: Cli) {
    let j = cli.json;
    let r: Result<Value> = (|| match cli.cmd {
        Cmd::Daemon { adapter, no_native_control, enabled_by_default, retention_days, mcp_http } => {
            let mcp_addr = mcp_http.filter(|s| !s.is_empty()).map(|s| http_mcp::parse_addr(&s)).transpose().map_err(AdlError::invalid)?;
            let adapter = platform::make_adapter(adapter)?;
            let store = AgentStore::open(paths::config_dir().join("agents.json"))?;
            let audit = Audit::open(paths::state_dir().join("audit"), retention_days);
            let cfg = Config { require_native_control: !no_native_control && !enabled_by_default, enabled_by_default, ..Config::default() };
            let broker = Arc::new(Broker::new(adapter, store, audit, cfg));
            control_web::init(broker.clone());
            if enabled_by_default {
                match broker.enable_unattended() {
                    Ok(_) => eprintln!("w3bs-desktop: agent access ENABLED by default (non-desktop instance)"),
                    Err(e) => eprintln!("w3bs-desktop: --enabled-by-default could not enable ({}); retry with `w3bs desktop enable`", e.message),
                }
            }
            // A busy port must not take the broker down with it: stdio MCP and the controls still work.
            if let Some(addr) = mcp_addr
                && let Err(e) = http_mcp::spawn(addr, true)
            {
                eprintln!("w3bs-desktop: MCP HTTP endpoint {addr} not started ({e}); stdio MCP still works");
            }
            server::serve(broker).map_err(|e| AdlError::new(ErrorCode::Blocked, e.to_string()))?;
            Ok(Value::Null)
        }
        Cmd::Tray { wait_for_broker: secs } => {
            wait_for_broker(secs);
            platform::run_tray().map_err(|e| AdlError::new(ErrorCode::Blocked, e.to_string()))?;
            Ok(Value::Null)
        }
        Cmd::Control { open: true, no_browser } => {
            let v = control_client()?.call("control.login", json!({"via": "cli"}))?;
            let url = v["url"].as_str().unwrap_or_default().to_string();
            if no_browser {
                return Ok(if j { v } else { json!(format!("{url}\n(one-time sign-in link, valid {} s)", v["expires_in_s"])) });
            }
            match control_web::open_url(&url) {
                Ok(()) => Ok(if j { json!({"opened": v["app"]}) } else { json!(format!("Opened the control app ({}). Closing it does not turn agent access off.", v["app"].as_str().unwrap_or_default())) }),
                Err(e) => Ok(json!(format!("{e}. Open this one-time link in your browser within {} s:\n{url}", v["expires_in_s"]))),
            }
        }
        Cmd::Control { .. } => {
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
            if ch.get("state").is_some() {
                return Ok(if j { ch } else { json!(status_line(&ch)) });
            }
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
        Cmd::Doctor { prompt } => control_client()?.call("doctor", json!({"prompt": prompt})),
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
                    Ok(json!({"agent": v["agent"], "token_file": path, "next": format!("w3bs desktop setup <claude-code|opencode|codex> --agent {name}")}))
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
        Cmd::Training(t) => {
            let mut c = control_client()?;
            let read = |p: &PathBuf| std::fs::read_to_string(p).map_err(|e| AdlError::invalid(format!("{}: {e}", p.display())));
            let v = match t {
                TrainingCmd::Status => c.call("training.status", json!({}))?,
                TrainingCmd::Apps => return c.call("training.apps", json!({})),
                TrainingCmd::Observe { apps, allow_agents } => c.call("training.start", json!({"mode": "observe", "applications": apps, "allow_agents": allow_agents}))?,
                TrainingCmd::Teach { goal_file, apps, allow_agents } => c.call("training.start", json!({"mode": "teach", "goal": read(&goal_file)?, "applications": apps, "allow_agents": allow_agents}))?,
                TrainingCmd::Pause => c.call("training.pause", json!({}))?,
                TrainingCmd::Resume => c.call("training.resume", json!({}))?,
                TrainingCmd::Mark => c.call("training.mark", json!({}))?,
                TrainingCmd::Explain { from_file } => c.call("training.explain", json!({"text": read(&from_file)?}))?,
                TrainingCmd::Finish { outcome } => {
                    let o = outcome.map(|o| match o {
                        TrainingOutcome::Success => "success",
                        TrainingOutcome::Partial => "partial",
                        TrainingOutcome::Failure => "failure",
                        TrainingOutcome::Interrupted => "interrupted",
                        TrainingOutcome::Unknown => "unknown",
                    });
                    return c.call("training.finish", json!({"outcome": o}));
                }
                TrainingCmd::Discard => c.call("training.discard", json!({}))?,
                TrainingCmd::Handoff => return c.call("training.handoff", json!({})),
                TrainingCmd::Forget { last } => {
                    let since = adl_broker::util::now_ms().saturating_sub(parse_last(&last)?);
                    let r = c.call("training.forget", json!({"since_ms": since}))?;
                    // What was already handed over lives in W3BS Training, which asks the person
                    // in this terminal to confirm.
                    if std::io::stdin().is_terminal() {
                        let bin = ["w3bs-training", "w3bs"].into_iter().find(|b| std::process::Command::new(b).arg("--help").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok());
                        match bin {
                            Some("w3bs-training") => drop(std::process::Command::new("w3bs-training").args(["forget", "--last", &last]).status()),
                            Some(_) => drop(std::process::Command::new("w3bs").args(["training", "forget", "--last", &last]).status()),
                            None => eprintln!("W3BS Training is not installed here; nothing else to delete."),
                        }
                    }
                    return Ok(r);
                }
                TrainingCmd::Open { no_browser } => {
                    let v = c.call("control.login", json!({"via": "cli"}))?;
                    let url = format!("{}#training", v["url"].as_str().unwrap_or_default());
                    if no_browser {
                        return Ok(json!(url));
                    }
                    return match control_web::open_url(&url) {
                        Ok(()) => Ok(json!("Opened Training in the control app.")),
                        Err(e) => Ok(json!(format!("{e}. Open this one-time link within {} s:\n{url}", v["expires_in_s"]))),
                    };
                }
            };
            Ok(if j { v } else { json!(training_line(&v)) })
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
        Cmd::Setup { client: SetupClient::GnomeExtension, .. } => {
            #[cfg(target_os = "linux")]
            {
                let v = gnome_ext::install()?;
                Ok(if j { v } else { json!(v["message"].as_str().unwrap_or_default()) })
            }
            #[cfg(not(target_os = "linux"))]
            Err(AdlError::unsupported("the GNOME Shell extension is Linux-only"))
        }
        Cmd::Setup { client, agent, http } => {
            let name = agent.unwrap_or_else(|| "claude-code".into());
            if !paths::token_file(&name).exists() {
                return Err(AdlError::not_found(format!("no saved token for {name}; run `w3bs desktop agents add {name}` first")));
            }
            let bin = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "w3bs-desktop".into());
            if let Some(url) = http {
                let tok = paths::token_file(&name).display().to_string();
                return Ok(json!(match client {
                    SetupClient::ClaudeCode => format!("claude mcp add --transport http w3bs-desktop {url} --header \"Authorization: Bearer $(cat '{tok}')\""),
                    SetupClient::Opencode => serde_json::to_string_pretty(&json!({"mcp": {"w3bs-desktop": {"type": "remote", "url": url, "headers": {"Authorization": format!("Bearer {{file:{tok}}}")}, "enabled": true}}})).unwrap(),
                    SetupClient::Codex => format!("# ~/.codex/config.toml (needs a Codex version with streamable HTTP MCP)\n[mcp_servers.w3bs-desktop]\nurl = \"{url}\"\nbearer_token_env_var = \"W3BS_DESKTOP_TOKEN\"\n# then: export W3BS_DESKTOP_TOKEN=$(cat '{tok}')"),
                    SetupClient::GnomeExtension => unreachable!("handled above"),
                }));
            }
            Ok(json!(match client {
                SetupClient::ClaudeCode => format!("claude mcp add w3bs-desktop -- {bin} mcp serve --stdio --agent {name}"),
                SetupClient::Opencode => serde_json::to_string_pretty(&json!({"mcp": {"w3bs-desktop": {"type": "local", "command": [bin, "mcp", "serve", "--stdio", "--agent", name], "enabled": true}}})).unwrap(),
                SetupClient::Codex => format!("# ~/.codex/config.toml\n[mcp_servers.w3bs-desktop]\ncommand = \"{bin}\"\nargs = [\"mcp\", \"serve\", \"--stdio\", \"--agent\", \"{name}\"]"),
                SetupClient::GnomeExtension => unreachable!("handled above"),
            }))
        }
    })();
    match r {
        Ok(Value::Null) => {}
        Ok(v) => print(&v, j),
        Err(e) => fail(e, j),
    }
}
