//! `w3bs os`: keep a W3BS OS machine (Alpine Linux) current over the network.
//!
//! - `status`  : edition, Alpine release and repository branch, the newest branch, pending
//!               package upgrades, running vs installed kernel, w3bs version.
//! - `update`  : `apk update` + `apk upgrade`, then `w3bs self update` and `w3bs update`.
//! - `upgrade` : a dist-upgrade to the next Alpine stable branch: snapshot (btrfs root),
//!               back up and rewrite /etc/apk/repositories, `apk upgrade --available`, with a
//!               journal so an interrupted upgrade can be resumed or its repositories rolled back.
//!
//! Reinstalling from the ISO is only for new installs. Privileged steps run through doas or
//! sudo when w3bs is not root; w3bs never asks for or stores a password itself.

use serde_json::{Value, json};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use w3bs_core::{Error, Interact, Result};
use w3bs_schema::result::{OperationResult, Overall};

pub const RELEASE_FILE: &str = "/etc/w3bs-os-release";
const ALPINE_RELEASE: &str = "/etc/alpine-release";
const REPOSITORIES: &str = "/etc/apk/repositories";
const JOURNAL: &str = "/var/lib/w3bs-os/upgrade.json";
const SNAPSHOTS: &str = "/.snapshots";
const MIN_FREE: u64 = 1 << 30;
const DEFAULT_MIRROR: &str = "https://dl-cdn.alpinelinux.org/alpine";
const DEFAULT_CHANNEL: &str = "https://w3bs.org/os/channel.json";
/// The repository components a rewrite touches: anything else is not an Alpine branch line.
const COMPONENTS: [&str; 3] = ["main", "community", "testing"];

fn fail(code: &'static str, msg: impl Into<String>) -> Error {
    Error::package(code, msg.into())
}

// ------------------------------------------------------------------ branches

/// An Alpine release branch: `v3.24` or `edge`. Stable branches sort below edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Branch {
    Stable(u32, u32),
    Edge,
}

impl Branch {
    /// `v3.24`, `3.24`, `3.24.2` (a release: its branch) or `edge`.
    pub fn parse(s: &str) -> Option<Branch> {
        let s = s.trim();
        if s == "edge" {
            return Some(Branch::Edge);
        }
        let mut it = s.strip_prefix('v').unwrap_or(s).split(['.', '_', '-']);
        let maj = it.next()?.parse().ok()?;
        let min = it.next()?.parse().ok()?;
        Some(Branch::Stable(maj, min))
    }

    /// How many stable releases apart `to` is from `self` (1 = the next one). Across a major
    /// version only `vN+1.0` counts as next; anything else is treated as a skip.
    pub fn steps_to(self, to: Branch) -> u32 {
        match (self, to) {
            (Branch::Stable(a, b), Branch::Stable(c, d)) if a == c => d.saturating_sub(b),
            (Branch::Stable(a, _), Branch::Stable(c, 0)) if c == a + 1 => 1,
            _ => u32::MAX,
        }
    }

    /// The next stable branch after `self`, given the newest released one.
    pub fn next(self, latest: Branch) -> Option<Branch> {
        match (self, latest) {
            (Branch::Stable(a, b), Branch::Stable(c, _)) if (a, b) < (c, u32::MAX) && latest > self => {
                if a == c { Some(Branch::Stable(a, b + 1)) } else { Some(Branch::Stable(a + 1, 0)) }
            }
            _ => None,
        }
    }
}

impl fmt::Display for Branch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Branch::Stable(a, b) => write!(f, "v{a}.{b}"),
            Branch::Edge => f.write_str("edge"),
        }
    }
}

/// The upgrade `w3bs os upgrade` should make.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Upgrade(Branch),
    UpToDate,
    /// The next branch exists but W3BS has not qualified it (channel.json `alpine_max`).
    HeldByPolicy { next: Branch, max: Branch },
}

pub struct Choice {
    pub from: Branch,
    pub latest: Option<Branch>,
    pub requested: Option<Branch>,
    pub max: Option<Branch>,
    pub allow_skip: bool,
    pub ignore_policy: bool,
    pub yes: bool,
}

/// Pick the target branch, refusing skips, downgrades, unqualified branches and an unasked
/// move to edge.
pub fn choose_target(c: &Choice) -> Result<Target> {
    if c.from == Branch::Edge {
        return Err(Error::usage("ON_EDGE", "this machine follows edge; there is no newer branch to upgrade to"));
    }
    let capped = |t: Branch| !c.ignore_policy && t != Branch::Edge && c.max.is_some_and(|m| t > m);
    match c.requested {
        Some(Branch::Edge) => {
            if !c.yes {
                return Err(Error::approval("EDGE_NEEDS_YES", "moving to edge (Alpine's rolling development branch) cannot be undone by another upgrade; confirm with --to edge --yes"));
            }
            Ok(Target::Upgrade(Branch::Edge))
        }
        Some(t) => {
            if t <= c.from {
                return Err(Error::usage("NOT_NEWER", format!("{t} is not newer than the current branch {}", c.from)));
            }
            if c.latest.is_some_and(|l| t > l) {
                return Err(fail("BRANCH_NOT_RELEASED", format!("{t} is not released yet (newest stable: {})", c.latest.unwrap())));
            }
            if c.from.steps_to(t) > 1 && !c.allow_skip {
                return Err(Error::usage("SKIPS_BRANCH", format!("{} to {t} skips a release; upgrade one branch at a time, or add --allow-skip", c.from)));
            }
            if capped(t) {
                return Err(fail("NOT_QUALIFIED", format!("W3BS has qualified Alpine up to {} so far (w3bs.org/os/channel.json); add --ignore-policy to upgrade to {t} anyway", c.max.unwrap())));
            }
            Ok(Target::Upgrade(t))
        }
        None => {
            let Some(latest) = c.latest else {
                return Err(fail("NO_LATEST_BRANCH", "could not find the newest Alpine branch; pass --to vX.Y").retryable());
            };
            match c.from.next(latest) {
                None => Ok(Target::UpToDate),
                Some(n) if capped(n) => Ok(Target::HeldByPolicy { next: n, max: c.max.unwrap() }),
                Some(n) => Ok(Target::Upgrade(n)),
            }
        }
    }
}

// ------------------------------------------------------------------ /etc/apk/repositories

/// Where a repository line names its branch: byte range of the segment and what it says.
#[derive(Debug, PartialEq, Eq)]
enum Seg {
    Branch(Branch),
    LatestStable,
}

/// Find `/<branch>/<component>` in a line, where branch is `vX.Y`, `latest-stable` or `edge`
/// and component is main, community or testing. Lines without one (local paths, other
/// distributions' repositories) are not Alpine branch lines.
fn branch_segment(line: &str) -> Option<(usize, usize, Seg)> {
    let b = line.as_bytes();
    let mut i = 0;
    while let Some(p) = line[i..].find('/') {
        let start = i + p + 1;
        let end = line[start..].find('/').map(|e| start + e)?;
        let seg = &line[start..end];
        let rest = &line[end + 1..];
        let comp_end = rest.find(|ch: char| ch == '/' || ch.is_whitespace()).unwrap_or(rest.len());
        if COMPONENTS.contains(&&rest[..comp_end]) {
            let kind = if seg == "latest-stable" {
                Some(Seg::LatestStable)
            } else if seg == "edge" || (seg.starts_with('v') && seg[1..].split('.').count() == 2 && seg[1..].split('.').all(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()))) {
                Branch::parse(seg).map(Seg::Branch)
            } else {
                None
            };
            if let Some(k) = kind {
                return Some((start, end, k));
            }
        }
        i = end;
        if i >= b.len() {
            break;
        }
    }
    None
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineChange {
    pub line: usize,
    pub before: String,
    pub after: String,
    pub comment: bool,
}

#[derive(Debug)]
pub struct Rewrite {
    pub text: String,
    pub changes: Vec<LineChange>,
    /// Active lines left as they are (local repositories, edge lines, other repositories).
    pub kept: Vec<String>,
}

impl Rewrite {
    pub fn active_changes(&self) -> usize {
        self.changes.iter().filter(|c| !c.comment).count()
    }
}

/// Point every Alpine branch line (`/vX.Y/` or `/latest-stable/`) at `to`, commented-out
/// lines included so uncommenting one later does not bring the old branch back. Edge lines are
/// left alone unless the target is edge (then they already match). Everything else is kept
/// byte for byte.
pub fn rewrite_repositories(src: &str, to: Branch) -> Rewrite {
    let mut changes = vec![];
    let mut kept = vec![];
    let mut out = String::with_capacity(src.len());
    for (n, raw) in src.split_inclusive('\n').enumerate() {
        let (line, nl) = raw.strip_suffix('\n').map(|l| (l, "\n")).unwrap_or((raw, ""));
        let mut new = line.to_owned();
        match branch_segment(line) {
            Some((s, e, Seg::LatestStable)) => new = format!("{}{to}{}", &line[..s], &line[e..]),
            Some((s, e, Seg::Branch(b))) if b != Branch::Edge && b != to => new = format!("{}{to}{}", &line[..s], &line[e..]),
            _ => {
                if !is_comment(line) && !line.trim().is_empty() {
                    kept.push(line.to_owned());
                }
            }
        }
        if new != line {
            changes.push(LineChange { line: n + 1, before: line.to_owned(), after: new.clone(), comment: is_comment(line) });
        }
        out.push_str(&new);
        out.push_str(nl);
    }
    Rewrite { text: out, changes, kept }
}

/// The branch the active repository lines follow: `Some(Ok(branch))`, `Some(Err(()))` for
/// latest-stable, `None` when no line names one.
pub fn repo_branch(src: &str) -> Option<std::result::Result<Branch, ()>> {
    src.lines().filter(|l| !is_comment(l)).find_map(branch_segment).map(|(_, _, s)| match s {
        Seg::Branch(b) => Ok(b),
        Seg::LatestStable => Err(()),
    })
}

/// The first active `main` repository URL (for checking the target branch is reachable).
fn main_repo_url(src: &str) -> Option<String> {
    src.lines().filter(|l| !is_comment(l)).find_map(|l| {
        let (_, e, _) = branch_segment(l)?;
        let url = l.split_whitespace().find(|w| w.starts_with("http://") || w.starts_with("https://"))?;
        (l[e + 1..].starts_with("main")).then(|| url.trim_end_matches('/').to_owned())
    })
}

// ------------------------------------------------------------------ KEY=VALUE files

pub fn parse_release(text: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(v);
            m.insert(k.trim().to_owned(), Value::String(v.to_owned()));
        }
    }
    m
}

/// Set keys in a KEY=VALUE file, keeping other lines and their order.
pub fn set_release_fields(text: &str, fields: &[(&str, String)]) -> String {
    let quote = |v: &str| if v.chars().any(|c| c.is_whitespace() || c == '"' || c == '$') { format!("\"{}\"", v.replace(['"', '$', '\\'], "")) } else { v.to_owned() };
    let mut done = vec![false; fields.len()];
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            if let Some((k, _)) = l.split_once('=')
                && let Some(i) = fields.iter().position(|(f, _)| *f == k.trim())
            {
                done[i] = true;
                return format!("{}={}", fields[i].0, quote(&fields[i].1));
            }
            l.to_owned()
        })
        .collect();
    for (i, (k, v)) in fields.iter().enumerate() {
        if !done[i] {
            out.push(format!("{k}={}", quote(v)));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// `(i/N) Upgrading x (a -> b)` lines from `apk upgrade --simulate`.
pub fn simulated_changes(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('(') && l.split_once(')').is_some_and(|(n, _)| n[1..].split('/').count() == 2 && n[1..].split('/').all(|x| x.bytes().all(|c| c.is_ascii_digit()) && !x.is_empty())))
        .map(|l| l.split_once(") ").map(|(_, r)| r.to_owned()).unwrap_or_else(|| l.to_owned()))
        .collect()
}

/// `branch: v3.24` (else `version: 3.24.2`) from latest-releases.yaml.
pub fn latest_from_yaml(y: &str) -> Option<Branch> {
    let field = |k: &str| y.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().trim_matches('"').to_owned()));
    field("branch:").and_then(|b| Branch::parse(&b)).or_else(|| field("version:").and_then(|v| Branch::parse(&v)))
}

// ------------------------------------------------------------------ the machine

fn read(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

fn is_root() -> bool {
    #[cfg(unix)]
    // SAFETY: geteuid has no preconditions.
    unsafe {
        libc::geteuid() == 0
    }
    #[cfg(not(unix))]
    false
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

pub fn in_container() -> bool {
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || std::env::var_os("container").is_some()
        || read("/proc/1/cgroup").is_some_and(|c| ["docker", "kubepods", "containerd", "lxc", "libpod"].iter().any(|k| c.contains(k)))
}

fn root_fstype() -> Option<String> {
    read("/proc/self/mounts")?.lines().filter_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 2 && f[1] == "/").then(|| f[2].to_owned())
    }).last()
}

fn free_bytes(path: &str) -> Option<u64> {
    #[cfg(unix)]
    {
        let c = std::ffi::CString::new(path).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c is a valid NUL-terminated path and s a valid out pointer.
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        Some(s.f_bavail as u64 * s.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn kernel() -> Value {
    let running = read("/proc/sys/kernel/osrelease").map(|s| s.trim().to_owned());
    let mut installed: Vec<String> = std::fs::read_dir("/lib/modules").map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    installed.sort();
    let container = in_container();
    let reboot = if container || installed.is_empty() { Value::Null } else { Value::Bool(running.as_ref().is_some_and(|r| !installed.contains(r))) };
    json!({ "running": running, "installed": installed, "reboot_needed": reboot, "applies": !container })
}

fn arch() -> String {
    Command::new("apk").arg("--print-arch").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| std::env::consts::ARCH.to_owned())
}

/// Runs commands, through doas or sudo for the privileged ones when w3bs is not root.
struct Sys {
    prefix: Option<Vec<String>>,
}

impl Sys {
    fn new(non_interactive: bool) -> Sys {
        let prefix = if is_root() {
            Some(vec![])
        } else {
            ["doas", "sudo"].iter().find(|b| on_path(b)).map(|b| {
                let mut p = vec![b.to_string()];
                if non_interactive {
                    p.push("-n".into());
                }
                p
            })
        };
        Sys { prefix }
    }

    fn require(&self) -> Result<()> {
        if self.prefix.is_none() {
            return Err(Error::preflight("NEEDS_ROOT", "this needs root: run it as root, or install doas (W3BS OS) or sudo"));
        }
        Ok(())
    }

    fn how(&self) -> &'static str {
        match self.prefix.as_deref() {
            Some([]) => "root",
            Some([b, ..]) if b == "doas" => "doas",
            Some(_) => "sudo",
            None => "none",
        }
    }

    fn cmd(&self, args: &[&str], privileged: bool) -> Command {
        let pre: Vec<String> = if privileged { self.prefix.clone().unwrap_or_default() } else { vec![] };
        let all: Vec<String> = pre.into_iter().chain(args.iter().map(|s| s.to_string())).collect();
        let mut c = Command::new(&all[0]);
        c.args(&all[1..]);
        c
    }

    /// Run with output streamed to stderr (stdout is reserved for the result).
    fn run(&self, args: &[&str], privileged: bool) -> Result<()> {
        let st = self.cmd(args, privileged).stdin(Stdio::inherit()).stdout(Stdio::from(std::io::stderr())).stderr(Stdio::inherit()).status();
        match st {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(fail("COMMAND_FAILED", format!("`{}` exited with {}", args.join(" "), s.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".into())))),
            Err(e) => Err(fail("COMMAND_FAILED", format!("`{}`: {e}", args.join(" ")))),
        }
    }

    fn output(&self, args: &[&str], privileged: bool) -> Result<String> {
        match self.cmd(args, privileged).stdin(Stdio::null()).output() {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(o) => Err(fail("COMMAND_FAILED", format!("`{}`: {}", args.join(" "), brief(&String::from_utf8_lossy(&o.stderr))))),
            Err(e) => Err(fail("COMMAND_FAILED", format!("`{}`: {e}", args.join(" ")))),
        }
    }

    /// Replace a root-owned file atomically (temp file + rename in the same directory).
    fn write(&self, path: &str, data: &str) -> Result<()> {
        if let Some(dir) = Path::new(path).parent() {
            self.run(&["mkdir", "-p", &dir.to_string_lossy()], true)?;
        }
        let mut c = self.cmd(&["sh", "-c", "cat > \"$1.w3bs-new\" && chmod 0644 \"$1.w3bs-new\" && mv -f \"$1.w3bs-new\" \"$1\"", "sh", path], true);
        let mut child = c.stdin(Stdio::piped()).stdout(Stdio::null()).spawn().map_err(|e| fail("WRITE_FAILED", format!("{path}: {e}")))?;
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("piped");
            stdin.write_all(data.as_bytes()).map_err(|e| fail("WRITE_FAILED", format!("{path}: {e}")))?;
        }
        match child.wait() {
            Ok(s) if s.success() => Ok(()),
            _ => Err(fail("WRITE_FAILED", format!("could not write {path}"))),
        }
    }
}

/// A command's stderr in one line: its ERROR lines when it printed any (apk does), else the last line.
pub fn brief(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let errors: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with("ERROR")).collect();
    if errors.is_empty() { lines.last().copied().unwrap_or("no output").to_owned() } else { errors.join("; ") }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(20)).user_agent(concat!("w3bs/", env!("CARGO_PKG_VERSION"))).build().expect("http client")
}

fn mirror() -> String {
    std::env::var("W3BS_ALPINE_MIRROR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_MIRROR.into()).trim_end_matches('/').to_owned()
}

/// The newest Alpine stable branch, from the mirror's latest-stable release file.
async fn latest_branch(arch: &str) -> std::result::Result<Branch, String> {
    let url = format!("{}/latest-stable/releases/{arch}/latest-releases.yaml", mirror());
    let r = http().get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("{url}: {e}"))?;
    let y = r.text().await.map_err(|e| format!("{url}: {e}"))?;
    latest_from_yaml(&y).ok_or_else(|| format!("{url}: no branch or version field"))
}

/// W3BS's qualification policy. A 404 (or anything that is not the JSON object) is no policy.
async fn policy() -> (Value, Option<String>) {
    let url = std::env::var("W3BS_OS_CHANNEL").unwrap_or_else(|_| DEFAULT_CHANNEL.into());
    if url.is_empty() || url == "off" {
        return (Value::Null, None);
    }
    match http().get(&url).send().await {
        Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => (Value::Null, None),
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) if v.is_object() => (v, None),
            _ => (Value::Null, Some(format!("{url} is not a JSON policy object; ignoring it"))),
        },
        Ok(r) => (Value::Null, Some(format!("{url}: HTTP {}; no W3BS upgrade policy applied", r.status()))),
        Err(e) => (Value::Null, Some(format!("{url}: {e}; no W3BS upgrade policy applied"))),
    }
}

async fn reachable(url: &str) -> bool {
    http().head(url).send().await.is_ok_and(|r| r.status().is_success())
}

fn pending(sys: &Sys, available: bool) -> std::result::Result<Vec<String>, String> {
    let mut args = vec!["apk", "upgrade", "--simulate"];
    if available {
        args.insert(2, "--available");
    }
    sys.output(&args, false).map(|o| simulated_changes(&o)).map_err(|e| e.message)
}

fn release_info() -> Value {
    read(RELEASE_FILE).map(|t| Value::Object(parse_release(&t))).unwrap_or(Value::Null)
}

fn alpine_release() -> Option<String> {
    read(ALPINE_RELEASE).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// The branch this machine is on: from the repositories file, latest-stable resolved through
/// /etc/alpine-release.
fn current_branch(repos: &str) -> Option<Branch> {
    match repo_branch(repos)? {
        Ok(b) => Some(b),
        Err(()) => alpine_release().and_then(|r| Branch::parse(&r)),
    }
}

fn require_alpine() -> Result<(String, String)> {
    let rel = alpine_release().ok_or_else(|| Error::preflight("NOT_ALPINE", "this is not Alpine Linux (no /etc/alpine-release); `w3bs os` manages W3BS OS and Alpine machines"))?;
    let repos = read(REPOSITORIES).ok_or_else(|| Error::preflight("NO_REPOSITORIES", format!("{REPOSITORIES} is missing")))?;
    Ok((rel, repos))
}

fn result(op: &str, data: Value, warnings: Vec<String>) -> OperationResult {
    let mut r = OperationResult::new(op);
    r.data = data;
    r.warnings = warnings;
    r
}

/// Mark a result failed while keeping its data (the steps that ran).
fn failed(mut r: OperationResult, e: &Error) -> OperationResult {
    r.result = if e.kind == w3bs_core::Kind::Package { Overall::Failure } else { e.overall() };
    r.error = Some(e.info());
    r
}

fn approve(ui: &dyn Interact, yes: bool, prompt: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    match ui.confirm(prompt) {
        Some(true) => Ok(()),
        Some(false) => Err(Error::approval("DECLINED", "not confirmed; nothing was changed")),
        None => Err(Error::approval("APPROVAL_REQUIRED", "re-run with --yes to approve, or interactively")),
    }
}

// ------------------------------------------------------------------ status

pub async fn status() -> Result<OperationResult> {
    let mut warnings = vec![];
    let rel = release_info();
    let alpine = alpine_release();
    let repos = read(REPOSITORIES).unwrap_or_default();
    let branch = current_branch(&repos);
    let sys = Sys::new(true);
    let arch = arch();
    let (latest, (pol, pol_warn)) = if alpine.is_some() { (latest_branch(&arch).await, policy().await) } else { (Err("not Alpine".into()), (Value::Null, None)) };
    warnings.extend(pol_warn);
    let latest = match latest {
        Ok(b) => Some(b),
        Err(e) => {
            if alpine.is_some() {
                warnings.push(format!("newest Alpine branch unknown: {e}"));
            }
            None
        }
    };
    let max = pol["alpine_max"].as_str().and_then(Branch::parse);
    let target = branch.and_then(|from| choose_target(&Choice { from, latest, requested: None, max, allow_skip: false, ignore_policy: false, yes: false }).ok());
    let pend = if alpine.is_some() {
        match pending(&sys, false) {
            Ok(p) => Value::from(p.len()),
            Err(e) => {
                warnings.push(format!("pending updates unknown: {e}{}", if e.contains("stale/unavailable") { " (no package index cached yet: run `apk update` as root, or `w3bs os update`)" } else { "" }));
                Value::Null
            }
        }
    } else {
        Value::Null
    };
    let journal = read(JOURNAL).and_then(|j| serde_json::from_str::<Value>(&j).ok()).unwrap_or(Value::Null);
    let w3bs_os = rel["ID"] == "w3bs-os";
    let k = kernel();
    Ok(result(
        "os.status",
        json!({
            "w3bs_os": w3bs_os,
            "release": rel,
            "edition": if w3bs_os { rel["EDITION"].clone() } else { Value::Null },
            "version": if w3bs_os { rel["VERSION"].clone() } else { Value::Null },
            "alpine_branch": branch.map(|b| b.to_string()),
            "alpine": { "release": alpine, "branch": branch.map(|b| b.to_string()), "repositories": REPOSITORIES, "arch": arch },
            "newest_branch": latest.map(|b| b.to_string()),
            "qualified_max": max.map(|b| b.to_string()),
            "upgrade_available": matches!(target, Some(Target::Upgrade(_))),
            "upgrade": match target {
                Some(Target::Upgrade(b)) => json!({ "available": b.to_string(), "command": format!("w3bs os upgrade --to {b}") }),
                Some(Target::HeldByPolicy { next, max }) => json!({ "available": Value::Null, "held": next.to_string(), "reason": format!("W3BS has qualified up to {max}") }),
                _ => json!({ "available": Value::Null }),
            },
            "pending_updates": pend,
            "reboot_needed": k["reboot_needed"].clone(),
            "kernel": k,
            "container": in_container(),
            "privilege": sys.how(),
            "w3bs_version": env!("CARGO_PKG_VERSION"),
            "journal": journal,
        }),
        warnings,
    ))
}

// ------------------------------------------------------------------ update

pub struct UpdateOpts {
    pub yes: bool,
    pub dry_run: bool,
    pub os_only: bool,
    pub non_interactive: bool,
}

fn step(name: &str, status: &str, detail: impl Into<Value>) -> Value {
    json!({ "step": name, "status": status, "detail": detail.into() })
}

/// Run `w3bs <args> --yes --json` (this binary) and summarize its result.
fn w3bs_child(args: &[&str]) -> (String, Value) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("w3bs"));
    let out = Command::new(exe).args(args).args(["--yes", "--json"]).stdin(Stdio::null()).stderr(Stdio::inherit()).output();
    let Ok(o) = out else { return ("failed".into(), "could not run w3bs".into()) };
    let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().unwrap_or_default();
    if code == "SELF_UPDATE_EXTERNAL" {
        return ("skipped".into(), "the w3bs CLI here is not managed by its own repository (installed by the OS image or another package manager)".into());
    }
    if o.status.success() {
        let pk = v["packages"].as_array().cloned().unwrap_or_default();
        let changed: Vec<String> = pk.iter().filter(|p| p["status"] == "updated" || p["status"] == "installed").map(|p| format!("{} {} -> {}", p["id"].as_str().unwrap_or("?"), p["from"].as_str().unwrap_or("-"), p["to"].as_str().unwrap_or("-"))).collect();
        return ("done".into(), if changed.is_empty() { "up to date".into() } else { json!(changed) });
    }
    ("failed".into(), v["error"]["message"].as_str().map(|m| format!("{m} ({code})")).unwrap_or_else(|| format!("exit {}", o.status.code().unwrap_or(1))).into())
}

pub fn update(o: &UpdateOpts, ui: &dyn Interact) -> Result<OperationResult> {
    let (rel, _) = require_alpine()?;
    let sys = Sys::new(o.non_interactive);
    let mut steps = vec![];
    let mut warnings = vec![];
    let mut res = result("os.update", Value::Null, vec![]);
    let set = |r: &mut OperationResult, steps: &Vec<Value>, extra: Value| {
        r.data = json!({ "alpine": rel, "dry_run": o.dry_run, "steps": steps, "kernel": kernel() });
        if let (Value::Object(d), Value::Object(x)) = (&mut r.data, extra) {
            d.extend(x);
        }
    };
    if !o.dry_run {
        sys.require()?;
        ui.progress("apk update");
        if let Err(e) = sys.run(&["apk", "update"], true) {
            steps.push(step("apk update", "failed", e.message.clone()));
            set(&mut res, &steps, json!({}));
            return Ok(failed(res, &e));
        }
        steps.push(step("apk update", "done", ""));
    } else {
        warnings.push("dry run: package list from the cached index (no `apk update`)".into());
    }
    let changes = match pending(&sys, false) {
        Ok(c) => c,
        Err(e) => {
            let e = fail("APK_SIMULATE", e);
            set(&mut res, &steps, json!({}));
            return Ok(failed(res, &e));
        }
    };
    let mut plan = vec![format!("apk upgrade: {} package(s)", changes.len())];
    plan.extend(changes.iter().map(|c| format!("  {c}")));
    if !o.os_only {
        plan.push("w3bs self update, then w3bs update (W3BS packages)".into());
    }
    if o.dry_run {
        steps.push(step("apk upgrade", "planned", json!(changes)));
        set(&mut res, &steps, json!({ "plan": plan }));
        res.warnings = warnings;
        return Ok(res);
    }
    if !changes.is_empty() || !o.os_only {
        for l in &plan {
            ui.progress(l);
        }
        if let Err(e) = approve(ui, o.yes, "Apply these updates?") {
            set(&mut res, &steps, json!({ "plan": plan }));
            return Ok(failed(res, &e));
        }
    }
    if changes.is_empty() {
        steps.push(step("apk upgrade", "skipped", "nothing to upgrade"));
    } else {
        ui.progress("apk upgrade");
        match sys.run(&["apk", "upgrade"], true) {
            Ok(()) => steps.push(step("apk upgrade", "done", json!(changes))),
            Err(e) => {
                steps.push(step("apk upgrade", "failed", e.message.clone()));
                set(&mut res, &steps, json!({ "plan": plan }));
                return Ok(failed(res, &e));
            }
        }
    }
    let mut any_failed = false;
    if !o.os_only {
        for (name, args) in [("w3bs self update", &["self", "update"][..]), ("w3bs update", &["update"][..])] {
            ui.progress(name);
            let (st, detail) = w3bs_child(args);
            any_failed |= st == "failed";
            steps.push(step(name, &st, detail));
        }
    }
    let k = kernel();
    if k["reboot_needed"] == true {
        warnings.push(format!("reboot to run the new kernel (running {})", k["running"].as_str().unwrap_or("?")));
    }
    set(&mut res, &steps, json!({ "plan": plan }));
    res.warnings = warnings;
    if any_failed {
        let e = fail("UPDATE_INCOMPLETE", "the system packages are updated, but a W3BS update step failed (see steps)");
        return Ok(failed(res, &e));
    }
    Ok(res)
}

// ------------------------------------------------------------------ upgrade

pub struct UpgradeOpts {
    pub to: Option<String>,
    pub allow_skip: bool,
    pub ignore_policy: bool,
    pub no_snapshot: bool,
    pub resume: bool,
    pub rollback: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub non_interactive: bool,
}

const STEPS: [&str; 9] = ["snapshot", "backup", "alpine-keys", "repositories", "index", "apk-tools", "upgrade", "release", "verify"];

fn load_journal() -> Option<Value> {
    read(JOURNAL).and_then(|j| serde_json::from_str(&j).ok())
}

fn save_journal(sys: &Sys, j: &Value) -> Result<()> {
    sys.write(JOURNAL, &(serde_json::to_string_pretty(j).unwrap() + "\n"))
}

fn stamp() -> String {
    // 2026-10-19T12:34:56Z -> 20261019T123456Z
    w3bs_core::now().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn rollback(o: &UpgradeOpts, ui: &dyn Interact) -> Result<OperationResult> {
    let sys = Sys::new(o.non_interactive);
    let Some(mut j) = load_journal() else {
        return Err(fail("NO_UPGRADE_JOURNAL", format!("no upgrade journal at {JOURNAL}; nothing to roll back")));
    };
    if j["state"] == "rolled_back" {
        return Err(fail("ALREADY_ROLLED_BACK", format!("the {} -> {} upgrade was already rolled back", j["from"].as_str().unwrap_or("?"), j["to"].as_str().unwrap_or("?"))));
    }
    let backup = j["backup"].as_str().unwrap_or_default().to_owned();
    let Some(saved) = read(&backup) else {
        return Err(fail("NO_BACKUP", format!("the repositories backup {backup} is missing")));
    };
    let snapshot = j["snapshot"].as_str().map(str::to_owned);
    let mut plan = vec![format!("restore {REPOSITORIES} from {backup} ({})", j["from"].as_str().unwrap_or("?"))];
    let advice = match &snapshot {
        Some(s) => format!("Packages already upgraded stay at {} versions until you boot the read-only snapshot {s} (pick it in the boot menu, or make it the default subvolume: btrfs subvolume snapshot {s} /.snapshots/restored && btrfs subvolume set-default /.snapshots/restored, then reboot). w3bs does not roll back a running root itself.", j["to"].as_str().unwrap_or("the new")),
        None => format!("No root snapshot was taken, so packages already upgraded stay at {} versions; restoring the old repositories alone does not downgrade them.", j["to"].as_str().unwrap_or("the new")),
    };
    plan.push(advice.clone());
    let mut res = result("os.upgrade", json!({ "action": "rollback", "from": j["from"], "to": j["to"], "plan": plan, "dry_run": o.dry_run, "snapshot": snapshot }), vec![]);
    if o.dry_run {
        return Ok(res);
    }
    sys.require()?;
    for l in &plan {
        ui.progress(l);
    }
    approve(ui, o.yes, "Restore the previous repositories?")?;
    if let Err(e) = sys.write(REPOSITORIES, &saved) {
        return Ok(failed(res, &e));
    }
    j["state"] = "rolled_back".into();
    j["rolled_back_at"] = w3bs_core::now().into();
    if let Err(e) = save_journal(&sys, &j) {
        res.warnings.push(format!("repositories restored, but the journal was not updated: {}", e.message));
    }
    res.data["restored"] = REPOSITORIES.into();
    res.data["next"] = advice.into();
    Ok(res)
}

pub async fn upgrade(o: &UpgradeOpts, ui: &dyn Interact) -> Result<OperationResult> {
    if o.rollback {
        return rollback(o, ui);
    }
    let (rel, repos) = require_alpine()?;
    let sys = Sys::new(o.non_interactive);
    let container = in_container();
    let mut warnings = vec![];
    let journal = load_journal();
    let unfinished = journal.as_ref().is_some_and(|j| j["state"] == "in_progress" || j["state"] == "failed");
    let arch = arch();

    // What to do: a fresh plan, or the journal's.
    let (from, to, mut j) = if o.resume {
        let Some(j) = journal.filter(|_| unfinished) else {
            return Err(fail("NOTHING_TO_RESUME", format!("no interrupted upgrade in {JOURNAL}")));
        };
        let from = j["from"].as_str().and_then(Branch::parse).ok_or_else(|| fail("BAD_JOURNAL", format!("{JOURNAL}: no from branch")))?;
        let to = j["to"].as_str().and_then(Branch::parse).ok_or_else(|| fail("BAD_JOURNAL", format!("{JOURNAL}: no to branch")))?;
        (from, to, j)
    } else {
        if unfinished {
            let j = journal.unwrap();
            return Err(Error::preflight("UPGRADE_INTERRUPTED", format!("the {} -> {} upgrade did not finish (step {}); run `w3bs os upgrade --resume` or `--rollback`", j["from"].as_str().unwrap_or("?"), j["to"].as_str().unwrap_or("?"), j["failed_step"].as_str().unwrap_or("unknown"))));
        }
        let from = current_branch(&repos).ok_or_else(|| Error::preflight("NO_BRANCH", format!("no Alpine branch line (…/vX.Y/main) in {REPOSITORIES}")))?;
        let requested = match &o.to {
            Some(t) => Some(Branch::parse(t).ok_or_else(|| Error::usage("BAD_BRANCH", format!("{t:?} is not a branch (vX.Y or edge)")))?),
            None => None,
        };
        let latest = match latest_branch(&arch).await {
            Ok(b) => Some(b),
            Err(e) => {
                warnings.push(format!("newest Alpine branch unknown: {e}"));
                None
            }
        };
        let (pol, w) = policy().await;
        warnings.extend(w);
        let max = pol["alpine_max"].as_str().and_then(Branch::parse);
        let target = choose_target(&Choice { from, latest, requested, max, allow_skip: o.allow_skip, ignore_policy: o.ignore_policy, yes: o.yes })?;
        let to = match target {
            Target::Upgrade(t) => t,
            Target::UpToDate => {
                let mut r = result("os.upgrade", json!({ "from": from.to_string(), "to": Value::Null, "newest_branch": latest.map(|b| b.to_string()), "message": format!("already on the newest stable branch ({from})") }), warnings);
                r.data["up_to_date"] = true.into();
                return Ok(r);
            }
            Target::HeldByPolicy { next, max } => {
                let r = result("os.upgrade", json!({ "from": from.to_string(), "to": Value::Null, "held": next.to_string(), "qualified_max": max.to_string(), "up_to_date": true, "message": format!("{next} is released but W3BS has qualified up to {max} so far; `--to {next} --ignore-policy` upgrades anyway") }), warnings);
                return Ok(r);
            }
        };
        let snapshot = if o.no_snapshot || container || root_fstype().as_deref() != Some("btrfs") { Value::Null } else { format!("{SNAPSHOTS}/w3bs-pre-upgrade-{from}-{}", stamp()).into() };
        let j = json!({
            "from": from.to_string(), "to": to.to_string(), "from_release": rel, "started_at": w3bs_core::now(),
            "state": "planned", "steps_done": [], "backup": format!("{REPOSITORIES}.bak-{from}"), "snapshot": snapshot,
            "version": pol["version"].as_str(),
        });
        (from, to, j)
    };

    // Preflight.
    let mut checks = vec![];
    let free = free_bytes("/");
    if let Some(f) = free {
        if f < MIN_FREE {
            return Err(Error::preflight("LOW_DISK", format!("{} MB free on /; a dist-upgrade needs at least 1024 MB", f >> 20)));
        }
        checks.push(format!("{} MB free on /", f >> 20));
    }
    if container {
        warnings.push("running in a container: the kernel and boot parts of the upgrade do not apply, and no snapshot is taken (rebuild or re-pull the image to upgrade those)".into());
    }
    let rw = rewrite_repositories(&repos, to);
    let done: Vec<String> = j["steps_done"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let repos_done = done.iter().any(|s| s == "repositories");
    if !repos_done && rw.active_changes() == 0 {
        return Err(Error::preflight("NO_BRANCH_LINES", format!("{REPOSITORIES} has no active Alpine branch lines to move to {to}")));
    }
    if !o.dry_run || !o.resume {
        let url = main_repo_url(if repos_done { &repos } else { &rw.text });
        if let Some(u) = url {
            let index = format!("{u}/{arch}/APKINDEX.tar.gz");
            if !reachable(&index).await {
                return Err(Error::preflight("TARGET_UNREACHABLE", format!("{index} is not reachable; check the network or the mirror")));
            }
            checks.push(format!("{to} index reachable: {index}"));
        }
    }
    let snapshot = j["snapshot"].as_str().map(str::to_owned);
    if snapshot.is_some() && !on_path("btrfs") {
        return Err(Error::preflight("NO_BTRFS_TOOL", "/ is btrfs but the btrfs tool is missing: `apk add btrfs-progs`, or pass --no-snapshot"));
    }
    if !o.dry_run {
        sys.require()?;
        if let Err(e) = sys.output(&["apk", "fix", "--simulate"], true) {
            return Err(Error::preflight("APK_DB", format!("the apk database is not healthy: {}", e.message)));
        }
        checks.push("apk database readable (apk fix --simulate)".into());
    }

    // The plan, exactly.
    let mut plan = vec![];
    match &snapshot {
        Some(s) => plan.push(format!("snapshot / read-only to {s}")),
        None if container => plan.push("no snapshot (container)".into()),
        None => plan.push(format!("no snapshot ({})", if o.no_snapshot { "--no-snapshot".into() } else { format!("/ is {}, not btrfs", root_fstype().unwrap_or_else(|| "unknown".into())) })),
    }
    plan.push(format!("back up {REPOSITORIES} to {}", j["backup"].as_str().unwrap_or_default()));
    plan.push("apk add -u alpine-keys (signing keys for the new branch)".into());
    for c in &rw.changes {
        plan.push(format!("{REPOSITORIES}:{}: {} -> {}", c.line, c.before, c.after));
    }
    for k in &rw.kept {
        plan.push(format!("{REPOSITORIES}: kept as is: {k}"));
    }
    plan.push("apk update".into());
    plan.push("apk add -u apk-tools".into());
    plan.push(format!("apk upgrade --available (every package to its {to} version)"));
    if Path::new(RELEASE_FILE).exists() {
        plan.push(format!("{RELEASE_FILE}: ALPINE, ALPINE_BRANCH={to}{}", j["version"].as_str().map(|v| format!(", VERSION={v}")).unwrap_or_default()));
    }
    plan.push(format!("journal: {JOURNAL}"));
    let plan: Vec<String> = if o.resume { plan.into_iter().chain([format!("resuming after: {}", if done.is_empty() { "nothing".into() } else { done.join(", ") })]).collect() } else { plan };
    let base = json!({ "action": if o.resume { "resume" } else { "upgrade" }, "from": from.to_string(), "to": to.to_string(), "from_release": rel, "plan": plan, "checks": checks, "dry_run": o.dry_run, "container": container, "journal": JOURNAL });
    let mut res = result("os.upgrade", base, warnings);
    if o.dry_run {
        return Ok(res);
    }
    if !o.non_interactive || o.yes {
        ui.progress(&format!("upgrade {from} -> {to}:"));
        for l in &plan {
            ui.progress(&format!("  {l}"));
        }
    }
    approve(ui, o.yes, &format!("Upgrade this machine from Alpine {from} to {to}?"))?;

    // Run, journaling each step.
    j["state"] = "in_progress".into();
    if let Err(e) = save_journal(&sys, &j) {
        return Ok(failed(res, &e));
    }
    let mut steps = vec![];
    for name in STEPS {
        if done.iter().any(|d| d == name) {
            steps.push(step(name, "done", "earlier run"));
            continue;
        }
        ui.progress(&format!("step {name}"));
        // Test hook: fail one step once, to exercise the journal and --resume.
        let inject = std::env::var("W3BS_OS_TEST_FAIL_AT").is_ok_and(|v| v == name);
        let r: Result<Value> = if inject { Err(fail("TEST_FAILURE", format!("simulated failure at step {name}"))) } else { match name {
            "snapshot" => match &snapshot {
                Some(s) => sys.run(&["mkdir", "-p", SNAPSHOTS], true).and_then(|_| sys.run(&["btrfs", "subvolume", "snapshot", "-r", "/", s], true)).map(|_| Value::from(s.clone())),
                None => Ok("skipped".into()),
            },
            "backup" => {
                let b = j["backup"].as_str().unwrap_or_default().to_owned();
                sys.run(&["cp", "-p", REPOSITORIES, &b], true).map(|_| Value::from(b))
            }
            "alpine-keys" => sys.run(&["apk", "add", "-u", "alpine-keys"], true).map(|_| Value::Null),
            "repositories" => {
                let cur = read(REPOSITORIES).unwrap_or_default();
                let rw = rewrite_repositories(&cur, to);
                sys.write(REPOSITORIES, &rw.text).map(|_| json!(rw.changes.iter().map(|c| c.after.clone()).collect::<Vec<_>>()))
            }
            "index" => sys.run(&["apk", "update"], true).map(|_| Value::Null),
            "apk-tools" => sys.run(&["apk", "add", "-u", "apk-tools"], true).map(|_| Value::Null),
            "upgrade" => sys.run(&["apk", "upgrade", "--available"], true).and_then(|_| sys.run(&["sync"], false)).map(|_| Value::Null),
            "release" => match read(RELEASE_FILE) {
                Some(t) => {
                    let mut f = vec![("ALPINE", alpine_release().unwrap_or_default()), ("ALPINE_BRANCH", to.to_string())];
                    if let Some(v) = j["version"].as_str() {
                        f.push(("VERSION", v.to_owned()));
                    }
                    let new = set_release_fields(&t, &f);
                    sys.write(RELEASE_FILE, &new).map(|_| Value::Object(parse_release(&new)))
                }
                None => Ok("skipped: not W3BS OS (no /etc/w3bs-os-release)".into()),
            },
            "verify" => {
                let now = alpine_release().unwrap_or_default();
                let ok = match to {
                    Branch::Stable(..) => Branch::parse(&now) == Some(to),
                    Branch::Edge => true,
                };
                if !ok {
                    Err(fail("VERIFY_FAILED", format!("{ALPINE_RELEASE} says {now}, not {to}")))
                } else {
                    match pending(&sys, true) {
                        Ok(left) if left.is_empty() => Ok(json!({ "alpine": now, "left": 0 })),
                        Ok(left) => {
                            res.warnings.push(format!("{} package(s) still differ from {to}: run `w3bs os upgrade --resume` or `apk upgrade --available`", left.len()));
                            Ok(json!({ "alpine": now, "left": left }))
                        }
                        Err(e) => Err(fail("VERIFY_FAILED", e)),
                    }
                }
            }
            _ => unreachable!(),
        } };
        match r {
            Ok(detail) => {
                let st = if detail.as_str().is_some_and(|d| d.starts_with("skipped")) { "skipped" } else { "done" };
                steps.push(step(name, st, detail));
                if let Some(a) = j["steps_done"].as_array_mut() {
                    a.push(name.into());
                }
                j["failed_step"] = Value::Null;
                if let Err(e) = save_journal(&sys, &j) {
                    res.data["steps"] = json!(steps);
                    return Ok(failed(res, &e));
                }
            }
            Err(e) => {
                steps.push(step(name, "failed", e.message.clone()));
                j["state"] = "failed".into();
                j["failed_step"] = name.into();
                j["error"] = e.message.clone().into();
                let _ = save_journal(&sys, &j);
                res.data["steps"] = json!(steps);
                res.data["next"] = "fix the cause, then `w3bs os upgrade --resume`; or `w3bs os upgrade --rollback` to restore the previous repositories".into();
                return Ok(failed(res, &e));
            }
        }
    }
    j["state"] = "done".into();
    j["finished_at"] = w3bs_core::now().into();
    let _ = save_journal(&sys, &j);
    res.data["steps"] = json!(steps);
    res.data["alpine"] = alpine_release().into();
    let k = kernel();
    if k["reboot_needed"] == true {
        res.warnings.push("reboot now to start the new kernel".into());
    }
    res.data["kernel"] = k;
    if let Some(s) = &snapshot {
        res.data["snapshot"] = s.clone().into();
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Branch {
        Branch::parse(s).unwrap()
    }

    #[test]
    fn branches_parse_and_order() {
        assert_eq!(b("v3.24"), Branch::Stable(3, 24));
        assert_eq!(b("3.24.2"), Branch::Stable(3, 24));
        assert_eq!(b("3.25_alpha20261001"), Branch::Stable(3, 25));
        assert_eq!(b("edge"), Branch::Edge);
        assert!(Branch::parse("latest").is_none());
        assert!(Branch::parse("v3").is_none());
        assert!(b("v3.9") < b("v3.10"));
        assert!(b("v3.24") < b("v4.0"));
        assert!(b("v4.0") < b("edge"));
        assert_eq!(b("v3.24").to_string(), "v3.24");
        assert_eq!(b("v3.23").steps_to(b("v3.24")), 1);
        assert_eq!(b("v3.22").steps_to(b("v3.24")), 2);
        assert_eq!(b("v3.24").steps_to(b("v4.0")), 1);
        assert_eq!(b("v3.24").steps_to(b("v4.1")), u32::MAX);
        assert_eq!(b("v3.23").next(b("v3.24")), Some(b("v3.24")));
        assert_eq!(b("v3.22").next(b("v3.24")), Some(b("v3.23")));
        assert_eq!(b("v3.24").next(b("v3.24")), None);
        assert_eq!(b("v3.24").next(b("v4.1")), Some(b("v4.0")));
    }

    fn choice(from: &str, latest: &str) -> Choice {
        Choice { from: b(from), latest: Some(b(latest)), requested: None, max: None, allow_skip: false, ignore_policy: false, yes: false }
    }

    #[test]
    fn target_selection() {
        assert_eq!(choose_target(&choice("v3.23", "v3.24")).unwrap(), Target::Upgrade(b("v3.24")));
        assert_eq!(choose_target(&choice("v3.21", "v3.24")).unwrap(), Target::Upgrade(b("v3.22")));
        assert_eq!(choose_target(&choice("v3.24", "v3.24")).unwrap(), Target::UpToDate);
        // Skips need --allow-skip.
        let mut c = choice("v3.22", "v3.24");
        c.requested = Some(b("v3.24"));
        assert_eq!(choose_target(&c).unwrap_err().code, "SKIPS_BRANCH");
        c.allow_skip = true;
        assert_eq!(choose_target(&c).unwrap(), Target::Upgrade(b("v3.24")));
        // No downgrades, nothing unreleased.
        c.requested = Some(b("v3.21"));
        assert_eq!(choose_target(&c).unwrap_err().code, "NOT_NEWER");
        c.requested = Some(b("v3.25"));
        assert_eq!(choose_target(&c).unwrap_err().code, "BRANCH_NOT_RELEASED");
        // Edge only with --to edge --yes.
        let mut e = choice("v3.24", "v3.24");
        e.requested = Some(Branch::Edge);
        assert_eq!(choose_target(&e).unwrap_err().code, "EDGE_NEEDS_YES");
        e.yes = true;
        assert_eq!(choose_target(&e).unwrap(), Target::Upgrade(Branch::Edge));
        let mut on_edge = choice("v3.24", "v3.24");
        on_edge.from = Branch::Edge;
        assert_eq!(choose_target(&on_edge).unwrap_err().code, "ON_EDGE");
        // Latest unknown: only an explicit --to works.
        let mut n = choice("v3.23", "v3.24");
        n.latest = None;
        assert_eq!(choose_target(&n).unwrap_err().code, "NO_LATEST_BRANCH");
        n.requested = Some(b("v3.24"));
        assert_eq!(choose_target(&n).unwrap(), Target::Upgrade(b("v3.24")));
    }

    #[test]
    fn policy_caps_unqualified_branches() {
        let mut c = choice("v3.24", "v3.25");
        c.max = Some(b("v3.24"));
        assert_eq!(choose_target(&c).unwrap(), Target::HeldByPolicy { next: b("v3.25"), max: b("v3.24") });
        c.requested = Some(b("v3.25"));
        assert_eq!(choose_target(&c).unwrap_err().code, "NOT_QUALIFIED");
        c.ignore_policy = true;
        assert_eq!(choose_target(&c).unwrap(), Target::Upgrade(b("v3.25")));
        // A cap at or above the target does nothing.
        let mut d = choice("v3.23", "v3.25");
        d.max = Some(b("v3.24"));
        assert_eq!(choose_target(&d).unwrap(), Target::Upgrade(b("v3.24")));
        // Edge is an explicit opt-in, not capped.
        let mut e = choice("v3.24", "v3.25");
        e.max = Some(b("v3.24"));
        e.requested = Some(Branch::Edge);
        e.yes = true;
        assert_eq!(choose_target(&e).unwrap(), Target::Upgrade(Branch::Edge));
    }

    const REPOS: &str = "\
# Alpine repositories, written by the W3BS installer
https://dl-cdn.alpinelinux.org/alpine/v3.23/main
http://mirror.example.org/pub/alpine/v3.23/community
#https://dl-cdn.alpinelinux.org/alpine/v3.23/testing
@edge https://dl-cdn.alpinelinux.org/alpine/edge/main
@testing https://dl-cdn.alpinelinux.org/alpine/edge/testing
/media/usb/apks
http://mirror.postmarketos.org/postmarketos/v24.12
https://alpine.example/latest-stable/community/

# a comment that mentions v3.23 in prose
";

    #[test]
    fn rewrite_moves_branch_lines_only() {
        let r = rewrite_repositories(REPOS, b("v3.24"));
        let want = "\
# Alpine repositories, written by the W3BS installer
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
http://mirror.example.org/pub/alpine/v3.24/community
#https://dl-cdn.alpinelinux.org/alpine/v3.24/testing
@edge https://dl-cdn.alpinelinux.org/alpine/edge/main
@testing https://dl-cdn.alpinelinux.org/alpine/edge/testing
/media/usb/apks
http://mirror.postmarketos.org/postmarketos/v24.12
https://alpine.example/v3.24/community/

# a comment that mentions v3.23 in prose
";
        assert_eq!(r.text, want);
        assert_eq!(r.active_changes(), 3);
        assert_eq!(r.changes.len(), 4);
        assert!(r.changes.iter().any(|c| c.comment && c.line == 4));
        assert_eq!(r.kept, vec!["@edge https://dl-cdn.alpinelinux.org/alpine/edge/main", "@testing https://dl-cdn.alpinelinux.org/alpine/edge/testing", "/media/usb/apks", "http://mirror.postmarketos.org/postmarketos/v24.12"]);
        // Idempotent.
        let again = rewrite_repositories(&r.text, b("v3.24"));
        assert_eq!(again.text, r.text);
        assert_eq!(again.active_changes(), 0);
    }

    #[test]
    fn rewrite_to_edge_and_without_trailing_newline() {
        let r = rewrite_repositories("https://dl-cdn.alpinelinux.org/alpine/v3.24/main\nhttps://dl-cdn.alpinelinux.org/alpine/edge/testing", Branch::Edge);
        assert_eq!(r.text, "https://dl-cdn.alpinelinux.org/alpine/edge/main\nhttps://dl-cdn.alpinelinux.org/alpine/edge/testing");
        assert_eq!(r.active_changes(), 1);
    }

    #[test]
    fn current_branch_and_main_url() {
        assert_eq!(repo_branch(REPOS), Some(Ok(b("v3.23"))));
        assert_eq!(repo_branch("https://x/alpine/latest-stable/main\n"), Some(Err(())));
        assert_eq!(repo_branch("#https://x/alpine/v3.20/main\n/media/cdrom/apks\n"), None);
        assert_eq!(main_repo_url(REPOS).as_deref(), Some("https://dl-cdn.alpinelinux.org/alpine/v3.23/main"));
        assert_eq!(main_repo_url("@t https://m/alpine/v3.24/community\nhttps://m/alpine/v3.24/main/\n").as_deref(), Some("https://m/alpine/v3.24/main"));
    }

    #[test]
    fn release_file_fields() {
        let t = "# W3BS OS\nID=w3bs-os\nVERSION=\"0.1.0\"\nEDITION=iso\n";
        let m = parse_release(t);
        assert_eq!(m["ID"], "w3bs-os");
        assert_eq!(m["VERSION"], "0.1.0");
        let n = set_release_fields(t, &[("VERSION", "0.2.0".into()), ("ALPINE", "3.24.2".into()), ("ALPINE_BRANCH", "v3.24".into())]);
        assert_eq!(n, "# W3BS OS\nID=w3bs-os\nVERSION=0.2.0\nEDITION=iso\nALPINE=3.24.2\nALPINE_BRANCH=v3.24\n");
    }

    #[test]
    fn brief_errors() {
        assert_eq!(brief("WARNING: opening from cache x\nERROR: Not continuing due to stale/unavailable repositories.\n"), "ERROR: Not continuing due to stale/unavailable repositories.");
        assert_eq!(brief("one\ntwo\n\n"), "two");
        assert_eq!(brief(""), "no output");
    }

    #[test]
    fn simulate_and_yaml_parsing() {
        let out = "(1/3) Upgrading libcrypto3 (3.5.8-r0 -> 3.5.9-r0)\n(2/3) Upgrading libssl3 (3.5.8-r0 -> 3.5.9-r0)\n(3/3) Purging foo (1-r0)\nOK: 8206 KiB in 16 packages\n";
        assert_eq!(simulated_changes(out), vec!["Upgrading libcrypto3 (3.5.8-r0 -> 3.5.9-r0)", "Upgrading libssl3 (3.5.8-r0 -> 3.5.9-r0)", "Purging foo (1-r0)"]);
        assert!(simulated_changes("OK: 8206 KiB in 16 packages\n").is_empty());
        assert_eq!(latest_from_yaml("---\n-\n  title: \"Mini\"\n  branch: v3.24\n  version: 3.24.2\n"), Some(b("v3.24")));
        assert_eq!(latest_from_yaml("---\n-\n  version: 3.25.0\n"), Some(b("v3.25")));
        assert_eq!(latest_from_yaml("<html>"), None);
    }
}
