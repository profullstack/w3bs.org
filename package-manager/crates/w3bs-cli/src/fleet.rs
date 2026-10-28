//! `w3bs fleet`: this machine as part of your fleet on w3bs.org/fleet.
//!
//! `w3bs fleet join <token>` enrolls the machine with a one-time token made on
//! w3bs.org/fleet and installs a systemd service running `w3bs fleet agent`, which reports
//! the machine's health every 30 seconds and runs the few actions the owner may queue from
//! the page. Actions are a fixed allowlist (refresh, update, restart-desktop), run only when
//! the machine joined with actions allowed; nothing else is ever executed. The machine's
//! own credential (an agent key) lives in `$XDG_CONFIG_HOME/w3bs/fleet.json` (0600).
//!
//! `w3bs fleet ls / run / rm` act on the whole fleet with the account from `w3bs login`.
//!
//! `w3bs fleet inventory import <file>` stores the account's OpenDevice inventory
//! (logicsrc.com/docs/opendevice) on w3bs.org: every device it runs, agent or not, with its
//! monthly cost. `w3bs fleet cost` lists the merged fleet and the monthly total; the server
//! reads the inventory with @profullstack/fleet.
//!
//! `w3bs fleet screen on` lets the owner watch this machine's desktop from the page; see
//! `screen.rs`. `w3bs fleet view <machine>` opens that page.

use crate::auth;
use crate::screen::{self, Screen};
use serde_json::{Map, Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use w3bs_core::{Error, Result};

pub const UNIT: &str = "w3bs-fleet.service";
/// Actions the agent will run, and nothing else.
pub const ACTIONS: &[&str] = &["refresh", "update", "restart-desktop"];
/// Exit code of `w3bs fleet agent` when the server no longer knows this machine; the unit
/// does not restart on it.
pub const EXIT_REMOVED: i32 = 3;
const OUTPUT_LIMIT: usize = 16 * 1024;
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const ACTION_TIMEOUT: Duration = Duration::from_secs(120);

fn http() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().expect("http client")
}

// ------------------------------------------------------------------ local config

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub origin: String,
    pub machine_id: String,
    pub agent_key: String,
    pub name: String,
    pub allow_actions: bool,
    pub interval: u64,
    /// Screen sharing, set on this machine only (`w3bs fleet screen on|off`).
    pub screen: Option<Screen>,
}

impl Config {
    fn to_json(&self) -> Value {
        json!({
            "origin": self.origin,
            "machine_id": self.machine_id,
            "agent_key": self.agent_key,
            "name": self.name,
            "allow_actions": self.allow_actions,
            "interval": self.interval,
            "screen": self.screen.as_ref().map(Screen::to_json),
        })
    }
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            origin: v["origin"].as_str()?.to_owned(),
            machine_id: v["machine_id"].as_str()?.to_owned(),
            agent_key: v["agent_key"].as_str()?.to_owned(),
            name: v["name"].as_str().unwrap_or_default().to_owned(),
            allow_actions: v["allow_actions"].as_bool().unwrap_or(false),
            interval: clamp_interval(v["interval"].as_u64()),
            screen: Screen::from_json(&v["screen"]),
        })
    }
}

/// `$XDG_CONFIG_HOME/w3bs/fleet.json`, next to auth.json.
pub fn config_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    Ok(auth::auth_path()?.with_file_name("fleet.json"))
}

pub fn load_config(path: &Path) -> Result<Option<Config>> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| Config::from_json(&v))
            .map(Some)
            .ok_or_else(|| Error::preflight("FLEET_FILE", format!("{} is not a fleet config; run `w3bs fleet leave` or remove it", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::preflight("FLEET_FILE", format!("{}: {e}", path.display()))),
    }
}

fn require_config(path: &Path) -> Result<Config> {
    load_config(path)?.ok_or_else(|| Error::package("NOT_IN_FLEET", "This machine has not joined a fleet. Get a join command on w3bs.org/fleet, then run: w3bs fleet join <token>"))
}

/// Atomic write (temp file + rename), file 0600, directory 0700.
pub fn save_config(path: &Path, c: &Config) -> Result<()> {
    let io = |e: std::io::Error| Error::preflight("FLEET_FILE", format!("{}: {e}", path.display()));
    let dir = path.parent().ok_or_else(|| Error::preflight("FLEET_FILE", "fleet file has no directory"))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(io)?;
    let tmp = dir.join(format!(".fleet.json.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&c.to_json()).expect("json").as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// The server's interval, kept between 10 seconds and 5 minutes (30 when absent).
pub fn clamp_interval(v: Option<u64>) -> u64 {
    v.unwrap_or(30).clamp(10, 300)
}

// ------------------------------------------------------------------ machine info

/// (total, available) bytes from /proc/meminfo.
pub fn parse_meminfo(s: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        s.lines().find_map(|l| {
            let rest = l.strip_prefix(name)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse::<u64>().ok().map(|kb| kb * 1024)
        })
    };
    Some((field("MemTotal")?, field("MemAvailable")?))
}

pub fn parse_loadavg(s: &str) -> Option<[f64; 3]> {
    let mut it = s.split_whitespace().map(|x| x.parse::<f64>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

/// PRETTY_NAME from /etc/os-release, unquoted.
pub fn parse_os_release(s: &str) -> Option<String> {
    s.lines().find_map(|l| {
        let v = l.strip_prefix("PRETTY_NAME=")?.trim();
        let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(v);
        (!v.is_empty()).then(|| v.to_owned())
    })
}

pub fn parse_uptime(s: &str) -> Option<u64> {
    s.split_whitespace().next()?.parse::<f64>().ok().map(|f| f as u64)
}

fn read(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

fn command_out(program: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|s| !s.is_empty())
}

#[cfg(unix)]
fn disk(mount: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(mount).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a valid NUL-terminated path and st is a properly sized out-parameter.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = if st.f_frsize > 0 { st.f_frsize as u64 } else { st.f_bsize as u64 };
    Some((st.f_blocks as u64 * frsize, st.f_bavail as u64 * frsize))
}
#[cfg(not(unix))]
fn disk(_: &str) -> Option<(u64, u64)> {
    None
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// What the agent reports. Fields this platform cannot provide are left out.
pub fn collect_info(screen: Option<&Screen>) -> Value {
    let mut m = Map::new();
    let hostname = read("/proc/sys/kernel/hostname").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).or_else(|| command_out("hostname", &[]));
    if let Some(h) = hostname {
        m.insert("hostname".into(), h.into());
    }
    let os = read("/etc/os-release").and_then(|s| parse_os_release(&s)).or_else(|| {
        if cfg!(target_os = "macos") {
            command_out("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}"))
        } else {
            None
        }
    });
    if let Some(os) = os {
        m.insert("os".into(), os.into());
    }
    let kernel = read("/proc/sys/kernel/osrelease").map(|s| s.trim().to_owned()).or_else(|| command_out("uname", &["-r"]));
    if let Some(k) = kernel {
        m.insert("kernel".into(), k.into());
    }
    m.insert("arch".into(), std::env::consts::ARCH.into());
    if let Ok(n) = std::thread::available_parallelism() {
        m.insert("cpus".into(), n.get().into());
    }
    if let Some(u) = read("/proc/uptime").and_then(|s| parse_uptime(&s)) {
        m.insert("uptime_s".into(), u.into());
    }
    if let Some(l) = read("/proc/loadavg").and_then(|s| parse_loadavg(&s)) {
        m.insert("load".into(), json!(l));
    }
    if let Some((total, available)) = read("/proc/meminfo").and_then(|s| parse_meminfo(&s)) {
        m.insert("mem".into(), json!({ "total": total, "available": available }));
    }
    if let Some((total, available)) = disk("/") {
        m.insert("disk".into(), json!({ "mount": "/", "total": total, "available": available }));
    }
    m.insert("w3bs_version".into(), env!("CARGO_PKG_VERSION").into());
    let installed = on_path("w3bs-desktop") || home().is_some_and(|h| h.join(".local/bin/w3bs-desktop").is_file());
    let active = command_out("systemctl", &["--user", "is-active", "w3bs-desktop.service"]).is_some_and(|s| s == "active");
    m.insert("desktop".into(), json!({ "installed": installed, "active": active }));
    if let Ok(u) = std::env::var("USER") {
        m.insert("user".into(), u.into());
    }
    if let Some(s) = screen {
        m.insert("screen".into(), screen::info(s));
    }
    Value::Object(m)
}

// ------------------------------------------------------------------ actions

/// What an action resolved to; the runner below executes only these shapes.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Send a heartbeat now.
    Refresh,
    /// Run this exact program with these exact arguments.
    Run { program: String, args: Vec<String>, timeout: Duration },
    /// Refused, with the reason reported back as the failure output.
    Reject(String),
}

/// The allowlist. `exe` is this w3bs binary.
pub fn plan(allow_actions: bool, action: &str, exe: &str) -> Plan {
    if !ACTIONS.contains(&action) {
        return Plan::Reject(format!("unknown action {action:?}; allowed: {}", ACTIONS.join(", ")));
    }
    if !allow_actions {
        return Plan::Reject("actions not allowed on this machine".into());
    }
    match action {
        "refresh" => Plan::Refresh,
        "update" => Plan::Run { program: exe.into(), args: vec!["update".into(), "--yes".into(), "--non-interactive".into()], timeout: UPDATE_TIMEOUT },
        "restart-desktop" => Plan::Run { program: "systemctl".into(), args: vec!["--user".into(), "restart".into(), "w3bs-desktop.service".into()], timeout: ACTION_TIMEOUT },
        _ => unreachable!("checked against ACTIONS"),
    }
}

/// The result posted back: ("done" | "failed", output), plus whether to heartbeat now.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub status: &'static str,
    pub output: String,
    pub refresh: bool,
}

/// Plan and run one action. `runner(program, args, timeout)` returns (success, output).
pub fn dispatch<R>(allow_actions: bool, action: &str, exe: &str, mut runner: R) -> Outcome
where
    R: FnMut(&str, &[String], Duration) -> (bool, String),
{
    match plan(allow_actions, action, exe) {
        Plan::Refresh => Outcome { status: "done", output: "ok".into(), refresh: true },
        Plan::Reject(why) => Outcome { status: "failed", output: why, refresh: false },
        Plan::Run { program, args, timeout } => {
            let (ok, out) = runner(&program, &args, timeout);
            Outcome { status: if ok { "done" } else { "failed" }, output: tail(&out, OUTPUT_LIMIT), refresh: false }
        }
    }
}

/// The last `limit` bytes, cut on a character boundary.
pub fn tail(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_owned();
    }
    let mut start = s.len() - limit;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

/// Run a program with a deadline, capturing stdout then stderr.
fn run_with_timeout(program: &str, args: &[String], timeout: Duration) -> (bool, String) {
    use std::process::{Command, Stdio};
    let mut child = match Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("could not run {program}: {e}")),
    };
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let ho = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let he = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => break None,
        }
    };
    let mut out = String::from_utf8_lossy(&ho.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&he.join().unwrap_or_default()).into_owned();
    if !err.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&err);
    }
    match status {
        Some(s) => (s.success(), out),
        None => (false, format!("{out}\n(timed out after {}s)", timeout.as_secs())),
    }
}

// ------------------------------------------------------------------ systemd

/// The unit text. A system unit when installed as root, else a user unit.
pub fn unit_text(exe: &str, config: &Path, system: bool) -> String {
    let wanted = if system { "multi-user.target" } else { "default.target" };
    format!(
        "[Unit]\nDescription=W3BS fleet agent (reports this machine to w3bs.org/fleet)\nWants=network-online.target\nAfter=network-online.target\n\n[Service]\nExecStart={exe} fleet agent --config {config}\nRestart=on-failure\nRestartSec=10\nRestartPreventExitStatus={EXIT_REMOVED}\n\n[Install]\nWantedBy={wanted}\n",
        config = config.display(),
    )
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}
#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

fn system_unit_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system").join(UNIT)
}

fn user_unit_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()).map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("systemd/user").join(UNIT))
}

/// This binary at a path that survives `w3bs update`: ~/.local/bin/w3bs when present.
fn stable_exe() -> String {
    if let Some(p) = home().map(|h| h.join(".local/bin/w3bs")).filter(|p| p.exists()) {
        return p.display().to_string();
    }
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "w3bs".into())
}

fn systemctl(system: bool, args: &[&str]) -> std::result::Result<(), String> {
    let mut cmd = std::process::Command::new("systemctl");
    if !system {
        cmd.arg("--user");
    }
    match cmd.args(args).stdin(std::process::Stdio::null()).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

/// Install and start the agent service. Returns notes for the user (empty when all is well).
fn install_service(config: &Path) -> std::result::Result<Vec<String>, String> {
    let system = is_root();
    if systemctl(system, &["show-environment"]).is_err() {
        return Err("systemd is not available here".into());
    }
    let path = if system { system_unit_path() } else { user_unit_path().ok_or("HOME is not set")? };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, unit_text(&stable_exe(), config, system)).map_err(|e| format!("{}: {e}", path.display()))?;
    systemctl(system, &["daemon-reload"])?;
    systemctl(system, &["enable", UNIT])?;
    systemctl(system, &["restart", UNIT])?;
    let mut notes = Vec::new();
    if !system {
        let user = std::env::var("USER").unwrap_or_default();
        let linger = std::process::Command::new("loginctl").args(["enable-linger", &user]).stdin(std::process::Stdio::null()).output();
        if !matches!(linger, Ok(o) if o.status.success()) {
            notes.push(format!("the agent stops when you log out; ask an admin to run: loginctl enable-linger {user}"));
        }
    }
    Ok(notes)
}

/// Stop, disable and delete whichever unit is installed. Returns what was removed.
fn remove_service() -> Vec<String> {
    let mut removed = Vec::new();
    for (system, path) in [(true, Some(system_unit_path())), (false, user_unit_path())] {
        let Some(path) = path else { continue };
        if !path.exists() || (system && !is_root()) {
            continue;
        }
        let _ = systemctl(system, &["disable", "--now", UNIT]);
        if std::fs::remove_file(&path).is_ok() {
            let _ = systemctl(system, &["daemon-reload"]);
            removed.push(path.display().to_string());
        }
    }
    removed
}

// ------------------------------------------------------------------ HTTP

/// POST/GET/DELETE with optional bearer; returns (status, JSON body or Null).
async fn call(client: &reqwest::Client, method: reqwest::Method, url: &str, bearer: Option<&str>, body: Option<&Value>) -> Result<(u16, Value)> {
    let mut req = client.request(method, url);
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    if let Some(body) = body {
        req = req.json(body);
    }
    let resp = req.send().await.map_err(|e| Error::package("FLEET_UNAVAILABLE", format!("{url}: {e}")).retryable())?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn server_error(code: &'static str, status: u16, body: &Value) -> Error {
    let msg = body["error"]["message"].as_str().or_else(|| body["error"].as_str()).map(str::to_owned).unwrap_or_else(|| format!("the server answered {status}"));
    Error::package(code, msg)
}

// ------------------------------------------------------------------ commands

pub async fn join(origin: &str, token: &str, name: Option<&str>, no_service: bool, screen: Option<Screen>) -> Result<Value> {
    let path = config_path(None)?;
    if let Some(c) = load_config(&path)? {
        return Err(Error::usage("ALREADY_JOINED", format!("This machine is already in a fleet as {} on {}. Run `w3bs fleet leave` first.", c.name, c.origin)));
    }
    let client = http();
    let mut body = json!({ "token": token, "info": collect_info(screen.as_ref()) });
    if let Some(n) = name {
        body["name"] = n.into();
    }
    let (status, v) = call(&client, reqwest::Method::POST, &format!("{origin}/api/fleet/join"), None, Some(&body)).await?;
    if status != 200 {
        return Err(server_error("FLEET_JOIN_FAILED", status, &v));
    }
    let c = Config {
        origin: origin.to_owned(),
        machine_id: v["machine_id"].as_str().ok_or_else(|| Error::package("FLEET_JOIN_FAILED", "the server sent no machine_id"))?.to_owned(),
        agent_key: v["agent_key"].as_str().ok_or_else(|| Error::package("FLEET_JOIN_FAILED", "the server sent no agent_key"))?.to_owned(),
        name: v["name"].as_str().map(str::to_owned).or_else(|| name.map(str::to_owned)).unwrap_or_default(),
        allow_actions: v["allow_actions"].as_bool().unwrap_or(false),
        interval: clamp_interval(v["interval"].as_u64()),
        screen,
    };
    save_config(&path, &c)?;
    let (service, notes) = if no_service {
        ("skipped", vec![format!("run the agent yourself: {} fleet agent", stable_exe())])
    } else {
        match install_service(&path) {
            Ok(notes) => (if is_root() { "system" } else { "user" }, notes),
            Err(why) => ("none", vec![format!("could not install the service ({why}); run the agent yourself: {} fleet agent", stable_exe())]),
        }
    };
    Ok(json!({
        "origin": c.origin, "machine_id": c.machine_id, "name": c.name,
        "allow_actions": c.allow_actions, "interval": c.interval,
        "service": service, "notes": notes, "config": path,
        "screen": c.screen.as_ref().map(screen::info),
    }))
}

pub fn status(config: Option<&Path>) -> Result<Value> {
    let path = config_path(config)?;
    let c = require_config(&path)?;
    let unit = if system_unit_path().exists() {
        Some(system_unit_path())
    } else {
        user_unit_path().filter(|p| p.exists())
    };
    Ok(json!({
        "origin": c.origin, "machine_id": c.machine_id, "name": c.name,
        "allow_actions": c.allow_actions, "interval": c.interval,
        "config": path, "service": unit, "info": collect_info(c.screen.as_ref()),
    }))
}

/// The agent loop. Returns only on a fatal error; a removed machine exits the process.
pub async fn agent(config: Option<&Path>) -> Result<Value> {
    let path = config_path(config)?;
    let mut c = require_config(&path)?;
    let client = http();
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| stable_exe());
    let mut interval = c.interval;
    let mut backoff = 5u64;
    // The screen listener, restarted whenever `w3bs fleet screen` changes the config.
    let mut listener: Option<(Screen, tokio::task::JoinHandle<()>)> = None;
    eprintln!("w3bs fleet agent: {} reporting to {} every {interval}s", c.name, c.origin);
    loop {
        if let Ok(Some(now)) = load_config(&path) {
            c = now;
        }
        if listener.as_ref().map(|(s, _)| s) != c.screen.as_ref() {
            if let Some((_, task)) = listener.take() {
                task.abort();
                eprintln!("w3bs fleet agent: screen sharing changed");
            }
            if let Some(s) = c.screen.clone() {
                listener = Some((s.clone(), tokio::spawn(screen::listen(c.origin.clone(), c.agent_key.clone(), s))));
            }
        }
        let body = json!({ "info": collect_info(c.screen.as_ref()) });
        match call(&client, reqwest::Method::POST, &format!("{}/api/fleet/heartbeat", c.origin), Some(&c.agent_key), Some(&body)).await {
            Ok((200, v)) => {
                backoff = 5;
                interval = clamp_interval(v["interval"].as_u64().or(Some(interval)));
                let mut refresh = false;
                for a in v["actions"].as_array().cloned().unwrap_or_default() {
                    let (Some(id), Some(action)) = (a["id"].as_str().map(str::to_owned), a["action"].as_str().map(str::to_owned)) else { continue };
                    eprintln!("w3bs fleet agent: action {action} ({id})");
                    let allow = c.allow_actions;
                    let exe = exe.clone();
                    let outcome = tokio::task::spawn_blocking(move || dispatch(allow, &action, &exe, run_with_timeout))
                        .await
                        .unwrap_or(Outcome { status: "failed", output: "the action crashed".into(), refresh: false });
                    refresh |= outcome.refresh;
                    let result = json!({ "status": outcome.status, "output": outcome.output });
                    if let Err(e) = call(&client, reqwest::Method::POST, &format!("{}/api/fleet/actions/{id}/result", c.origin), Some(&c.agent_key), Some(&result)).await {
                        eprintln!("w3bs fleet agent: could not report action {id}: {}", e.message);
                    }
                }
                if refresh {
                    continue;
                }
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
            Ok((401, _)) => {
                eprintln!("This machine was removed from the fleet; run w3bs fleet join again");
                std::process::exit(EXIT_REMOVED);
            }
            Ok((status, v)) => {
                eprintln!("w3bs fleet agent: heartbeat failed: {}", server_error("FLEET_HEARTBEAT", status, &v).message);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
            Err(e) => {
                eprintln!("w3bs fleet agent: {}", e.message);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

pub async fn leave(config: Option<&Path>) -> Result<Value> {
    let path = config_path(config)?;
    let c = require_config(&path)?;
    let left = call(&http(), reqwest::Method::POST, &format!("{}/api/fleet/leave", c.origin), Some(&c.agent_key), None).await;
    let server = matches!(left, Ok((200, _)) | Ok((401, _)));
    let removed = remove_service();
    std::fs::remove_file(&path).map_err(|e| Error::preflight("FLEET_FILE", format!("{}: {e}", path.display())))?;
    Ok(json!({ "origin": c.origin, "name": c.name, "server": server, "service_removed": removed }))
}

/// `w3bs fleet screen on|off`: change screen sharing on this machine. `None` shows the
/// current setting, `Some(None)` turns it off. The running agent picks the change up on its next
/// report, so no restart is needed.
pub fn screen_set(config: Option<&Path>, change: Option<Option<Screen>>) -> Result<Value> {
    let path = config_path(config)?;
    let mut c = require_config(&path)?;
    if let Some(next) = change {
        if let Some(s) = &next {
            screen::check_addr(&s.vnc).map_err(|e| Error::usage("BAD_VNC_ADDRESS", e))?;
        }
        c.screen = next;
        save_config(&path, &c)?;
        screen::forget_probe();
    }
    Ok(json!({
        "name": c.name, "machine_id": c.machine_id, "origin": c.origin,
        "screen": c.screen.as_ref().map(screen::info),
        "url": format!("{}/fleet/{}/screen", c.origin, c.machine_id),
    }))
}

/// `w3bs fleet view <machine>`: the machine's screen page, opened in the browser when one is
/// at hand.
pub async fn view(origin: &str, machine: &str, open: bool) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let list = machines(&http(), origin, &token).await?;
    let m = find_machine(&list, machine)?;
    let url = format!("{origin}/fleet/{}/screen", m["id"].as_str().unwrap_or_default());
    let opened = open && open_browser(&url);
    Ok(json!({
        "machine": m["name"], "machine_id": m["id"], "url": url, "opened": opened,
        "screen": m["info"]["screen"], "listening": m["screen_listening"],
    }))
}

fn open_browser(url: &str) -> bool {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("xdg-open", vec![url])
    } else {
        return false;
    };
    std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok()
}

async fn machines(client: &reqwest::Client, origin: &str, token: &str) -> Result<Vec<Value>> {
    let (status, v) = call(client, reqwest::Method::GET, &format!("{origin}/api/fleet/machines"), Some(token), None).await?;
    match status {
        200 => Ok(v.as_array().cloned().or_else(|| v["machines"].as_array().cloned()).unwrap_or_default()),
        401 => Err(Error::package("NOT_SIGNED_IN", format!("Your session has ended. Run: w3bs login{}", if origin == "https://w3bs.org" { String::new() } else { format!(" --api {origin}") }))),
        _ => Err(server_error("FLEET_LIST_FAILED", status, &v)),
    }
}

pub async fn ls(origin: &str) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let list = machines(&http(), origin, &token).await?;
    Ok(json!({ "origin": origin, "machines": list }))
}

fn signed_out(origin: &str) -> Error {
    Error::package("NOT_SIGNED_IN", format!("Your session has ended. Run: w3bs login{}", if origin == "https://w3bs.org" { String::new() } else { format!(" --api {origin}") }))
}

/// Read an OpenDevice inventory file (or `-` for stdin) and store it as the account's.
/// The server validates it with @profullstack/fleet and answers with the errors if any.
pub async fn inventory_import(origin: &str, file: &Path) -> Result<Value> {
    let text = if file == Path::new("-") {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| Error::preflight("INVENTORY_FILE", format!("stdin: {e}")))?;
        s
    } else {
        std::fs::read_to_string(file).map_err(|e| Error::preflight("INVENTORY_FILE", format!("{}: {e}", file.display())))?
    };
    let doc: Value = serde_json::from_str(&text).map_err(|e| Error::usage("INVENTORY_NOT_JSON", format!("{}: not JSON ({e})", file.display())))?;
    if !doc["devices"].is_array() {
        return Err(Error::usage("INVALID_INVENTORY", format!("{}: an OpenDevice inventory needs a \"devices\" array (logicsrc.com/docs/opendevice)", file.display())));
    }
    let token = auth::access_token(origin).await?;
    let (status, v) = call(&http(), reqwest::Method::PUT, &format!("{origin}/api/fleet/inventory"), Some(&token), Some(&doc)).await?;
    match status {
        200 => Ok(json!({ "origin": origin, "file": file.display().to_string(), "devices": v["devices"], "warnings": v["warnings"], "summary": v["summary"] })),
        401 => Err(signed_out(origin)),
        _ => {
            let details: Vec<String> = v["error"]["details"]["errors"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            let mut e = server_error("INVENTORY_IMPORT_FAILED", status, &v);
            if !details.is_empty() {
                e = Error::package("INVALID_INVENTORY", format!("{}:\n  {}", file.display(), details.join("\n  ")));
            }
            Err(e)
        }
    }
}

/// The stored inventory document, whole.
pub async fn inventory_show(origin: &str) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let (status, v) = call(&http(), reqwest::Method::GET, &format!("{origin}/api/fleet/inventory"), Some(&token), None).await?;
    match status {
        200 => Ok(json!({ "origin": origin, "document": v })),
        401 => Err(signed_out(origin)),
        _ => Err(server_error("INVENTORY_SHOW_FAILED", status, &v)),
    }
}

pub async fn inventory_rm(origin: &str) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let (status, v) = call(&http(), reqwest::Method::DELETE, &format!("{origin}/api/fleet/inventory"), Some(&token), None).await?;
    match status {
        200 => Ok(json!({ "origin": origin, "removed": true })),
        401 => Err(signed_out(origin)),
        _ => Err(server_error("INVENTORY_RM_FAILED", status, &v)),
    }
}

/// The merged fleet: joined machines with their inventory devices, devices without an
/// agent, and the monthly run rate per currency (unstated costs listed, never guessed).
pub async fn cost(origin: &str) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let (status, v) = call(&http(), reqwest::Method::GET, &format!("{origin}/api/fleet"), Some(&token), None).await?;
    match status {
        200 => Ok(json!({ "origin": origin, "machines": v["machines"], "devices": v["devices"], "summary": v["summary"] })),
        401 => Err(signed_out(origin)),
        _ => Err(server_error("FLEET_COST_FAILED", status, &v)),
    }
}

/// One line of `w3bs fleet cost`: a device's monthly cost as text.
pub fn monthly_text(m: &Value) -> String {
    match m["state"].as_str() {
        Some("stated") => match (m["amount"].as_f64(), m["currency"].as_str()) {
            (Some(a), Some("USD")) => format!("${a:.2}"),
            (Some(a), Some("EUR")) => format!("\u{20ac}{a:.2}"),
            (Some(a), Some("GBP")) => format!("\u{a3}{a:.2}"),
            (Some(a), Some(c)) => format!("{a:.2} {c}"),
            _ => "-".into(),
        },
        Some("grouped") => "shared bill".into(),
        Some("once") => "one-off".into(),
        Some(other) => other.into(),
        None => "-".into(),
    }
}

/// A machine by exact id, else by unique name.
pub fn find_machine<'a>(list: &'a [Value], key: &str) -> Result<&'a Value> {
    if let Some(m) = list.iter().find(|m| m["id"].as_str() == Some(key)) {
        return Ok(m);
    }
    let named: Vec<&Value> = list.iter().filter(|m| m["name"].as_str() == Some(key)).collect();
    match named.len() {
        1 => Ok(named[0]),
        0 => Err(Error::usage("NO_SUCH_MACHINE", format!("no machine named or with id {key:?}; see `w3bs fleet ls`"))),
        _ => Err(Error::usage("AMBIGUOUS_MACHINE", format!("{} machines are named {key:?}; use the id from `w3bs fleet ls`", named.len()))),
    }
}

pub async fn run_action(origin: &str, machine: &str, action: &str) -> Result<Value> {
    if !ACTIONS.contains(&action) {
        return Err(Error::usage("UNKNOWN_ACTION", format!("unknown action {action:?}; allowed: {}", ACTIONS.join(", "))));
    }
    let token = auth::access_token(origin).await?;
    let client = http();
    let list = machines(&client, origin, &token).await?;
    let m = find_machine(&list, machine)?;
    let id = m["id"].as_str().unwrap_or_default();
    let (status, v) = call(&client, reqwest::Method::POST, &format!("{origin}/api/fleet/machines/{id}/actions"), Some(&token), Some(&json!({ "action": action }))).await?;
    if status != 200 {
        return Err(server_error("FLEET_ACTION_FAILED", status, &v));
    }
    Ok(json!({ "origin": origin, "machine": m["name"], "machine_id": id, "action": v }))
}

pub async fn rm(origin: &str, machine: &str) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let client = http();
    let list = machines(&client, origin, &token).await?;
    let m = find_machine(&list, machine)?;
    let id = m["id"].as_str().unwrap_or_default();
    let (status, v) = call(&client, reqwest::Method::DELETE, &format!("{origin}/api/fleet/machines/{id}"), Some(&token), None).await?;
    if !(200..300).contains(&status) {
        return Err(server_error("FLEET_REMOVE_FAILED", status, &v));
    }
    Ok(json!({ "origin": origin, "machine": m["name"], "machine_id": id, "removed": true }))
}

/// Seconds since an RFC 3339 timestamp, for "last seen" columns. Best effort.
pub fn ago(ts: &str) -> String {
    let Ok(t) = jiff_like_parse(ts) else { return ts.to_owned() };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let s = (now - t).max(0);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS(.fff)Z` to Unix seconds (UTC only; what the server sends).
fn jiff_like_parse(ts: &str) -> std::result::Result<i64, ()> {
    let b = ts.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || !ts.ends_with('Z') {
        return Err(());
    }
    let n = |r: std::ops::Range<usize>| ts.get(r).and_then(|x| x.parse::<i64>().ok()).ok_or(());
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Ok(days * 86400 + h * 3600 + mi * 60 + s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_loadavg_osrelease_uptime() {
        let mem = "MemTotal:       16303596 kB\nMemFree:          400000 kB\nMemAvailable:    8151798 kB\n";
        assert_eq!(parse_meminfo(mem), Some((16303596 * 1024, 8151798 * 1024)));
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/1043 12345\n"), Some([0.52, 0.58, 0.59]));
        assert_eq!(parse_loadavg("x"), None);
        let osr = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nID=ubuntu\n";
        assert_eq!(parse_os_release(osr).as_deref(), Some("Ubuntu 24.04.3 LTS"));
        assert_eq!(parse_os_release("PRETTY_NAME=Fedora\n").as_deref(), Some("Fedora"));
        assert_eq!(parse_os_release("ID=x\n"), None);
        assert_eq!(parse_uptime("350735.47 1234.5\n"), Some(350735));
    }

    #[test]
    fn monthly_text_never_guesses() {
        assert_eq!(monthly_text(&json!({ "amount": 149, "currency": "USD", "state": "stated" })), "$149.00");
        assert_eq!(monthly_text(&json!({ "amount": 10, "currency": "EUR", "state": "stated" })), "\u{20ac}10.00");
        assert_eq!(monthly_text(&json!({ "amount": 5.5, "currency": "CHF", "state": "stated" })), "5.50 CHF");
        assert_eq!(monthly_text(&json!({ "amount": null, "currency": null, "state": "unstated" })), "unstated");
        assert_eq!(monthly_text(&json!({ "amount": null, "state": "grouped" })), "shared bill");
        assert_eq!(monthly_text(&Value::Null), "-");
    }

    #[test]
    fn interval_is_clamped() {
        assert_eq!(clamp_interval(None), 30);
        assert_eq!(clamp_interval(Some(1)), 10);
        assert_eq!(clamp_interval(Some(30)), 30);
        assert_eq!(clamp_interval(Some(10_000)), 300);
    }

    #[test]
    fn allowlist_dispatcher() {
        let calls = std::cell::RefCell::new(Vec::new());
        let runner = |p: &str, a: &[String], t: Duration| {
            calls.borrow_mut().push((p.to_owned(), a.to_vec(), t));
            (true, "fine".to_owned())
        };
        // Not allowed on this machine: nothing runs.
        let o = dispatch(false, "update", "/x/w3bs", runner);
        assert_eq!(o, Outcome { status: "failed", output: "actions not allowed on this machine".into(), refresh: false });
        // Unknown actions are refused even when actions are allowed.
        let o = dispatch(true, "rm -rf /", "/x/w3bs", runner);
        assert_eq!(o.status, "failed");
        assert!(o.output.starts_with("unknown action"));
        // Refresh runs nothing and asks for a heartbeat.
        let o = dispatch(true, "refresh", "/x/w3bs", runner);
        assert_eq!(o, Outcome { status: "done", output: "ok".into(), refresh: true });
        assert!(calls.borrow().is_empty());
        // Update runs exactly this binary's update.
        let o = dispatch(true, "update", "/x/w3bs", runner);
        assert_eq!(o.status, "done");
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/x/w3bs");
        assert_eq!(calls[0].1, vec!["update", "--yes", "--non-interactive"]);
        assert_eq!(calls[0].2, UPDATE_TIMEOUT);
        let o = dispatch(true, "restart-desktop", "/x/w3bs", |p: &str, a: &[String], _| (false, format!("{p} {}", a.join(" "))));
        assert_eq!(o.status, "failed");
        assert_eq!(o.output, "systemctl --user restart w3bs-desktop.service");
    }

    #[test]
    fn output_tail_is_bounded_and_utf8_safe() {
        assert_eq!(tail("short", 16), "short");
        let long = "é".repeat(20_000);
        let t = tail(&long, OUTPUT_LIMIT);
        assert!(t.len() <= OUTPUT_LIMIT + "…".len() + 1);
        assert!(t.starts_with('…'));
    }

    #[test]
    fn config_round_trip_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w3bs/fleet.json");
        let c = Config { origin: "https://w3bs.org".into(), machine_id: "m1".into(), agent_key: "fm_secret".into(), name: "dev2".into(), allow_actions: true, interval: 30, screen: Some(Screen { mode: screen::Mode::View, vnc: "127.0.0.1:5901".into() }) };
        save_config(&path, &c).unwrap();
        assert_eq!(load_config(&path).unwrap(), Some(c));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(load_config(&dir.path().join("missing.json")).unwrap(), None);
        std::fs::write(dir.path().join("bad.json"), "[]").unwrap();
        assert!(load_config(&dir.path().join("bad.json")).is_err());
    }

    #[test]
    fn unit_files() {
        let user = unit_text("/home/a/.local/bin/w3bs", Path::new("/home/a/.config/w3bs/fleet.json"), false);
        assert!(user.contains("ExecStart=/home/a/.local/bin/w3bs fleet agent --config /home/a/.config/w3bs/fleet.json\n"));
        assert!(user.contains("WantedBy=default.target"));
        assert!(user.contains("RestartPreventExitStatus=3"));
        assert!(user.contains("Restart=on-failure"));
        let root = unit_text("/usr/local/bin/w3bs", Path::new("/root/.config/w3bs/fleet.json"), true);
        assert!(root.contains("WantedBy=multi-user.target"));
        assert!(root.contains("After=network-online.target"));
    }

    #[test]
    fn machine_lookup() {
        let list = vec![json!({"id":"a1","name":"dev2"}), json!({"id":"b2","name":"web"}), json!({"id":"c3","name":"web"})];
        assert_eq!(find_machine(&list, "a1").unwrap()["name"], "dev2");
        assert_eq!(find_machine(&list, "dev2").unwrap()["id"], "a1");
        assert_eq!(find_machine(&list, "web").unwrap_err().code, "AMBIGUOUS_MACHINE");
        assert_eq!(find_machine(&list, "c3").unwrap()["name"], "web");
        assert_eq!(find_machine(&list, "nope").unwrap_err().code, "NO_SUCH_MACHINE");
    }

    #[test]
    fn timestamps() {
        assert_eq!(jiff_like_parse("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(jiff_like_parse("2026-10-14T00:00:00.000Z"), Ok(1_791_936_000));
        assert!(jiff_like_parse("yesterday").is_err());
    }
}
