//! `w3bs os setup`: set up and maintain a W3BS OS box's accounts and services.
//!
//! It runs `root-w3bs-os` (catalog package `profullstack.com/root-w3bs-os`) as root with the
//! arguments given, after installing it through the engine when it is missing (exactly like
//! `w3bs install profullstack.com/root-w3bs-os`, publisher signature included) or updating it
//! when the catalog has a newer release. Privilege comes from being root, else doas, else
//! sudo; w3bs never asks for, passes or stores a password.

use serde_json::{Value, json};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use w3bs_core::{Engine, Error, InstallRequest, Interact, Result, UpdateRequest};
use w3bs_schema::result::{OperationResult, Overall};

pub const PACKAGE: &str = "profullstack.com/root-w3bs-os";
pub const SCRIPT: &str = "root-w3bs-os";
/// Test hook: the directories searched besides PATH (colon separated), instead of
/// ~/.local/bin and /usr/local/sbin.
const DIRS_ENV: &str = "W3BS_OS_SETUP_DIRS";

/// The w3bs flags `os setup` understands. Everything else is the script's.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Flags {
    pub yes: bool,
    pub dry_run: bool,
    pub print: bool,
    pub no_update: bool,
    pub force: bool,
    pub json: bool,
    pub non_interactive: bool,
    pub quiet: bool,
    pub verbose: bool,
}

/// The arguments after `os setup` in argv (global flags may sit between `os` and `setup`).
pub fn tail(argv: &[String]) -> Vec<String> {
    let Some(os) = argv.iter().skip(1).position(|a| a == "os").map(|i| i + 1) else { return vec![] };
    match argv[os + 1..].iter().position(|a| a == "setup") {
        Some(i) => argv[os + 2 + i..].to_vec(),
        None => vec![],
    }
}

/// Split `os setup`'s arguments into w3bs flags and the script's arguments. Before the first
/// `--`, w3bs's own flags are taken wherever they appear and every other word is the
/// script's, in order; after `--`, everything is the script's verbatim (a later `--`
/// included).
pub fn split_args(raw: &[String]) -> (Flags, Vec<String>) {
    let mut f = Flags::default();
    let mut pass = vec![];
    let mut it = raw.iter();
    for a in it.by_ref() {
        match a.as_str() {
            "--" => break,
            "-y" | "--yes" => f.yes = true,
            "-n" | "--dry-run" => f.dry_run = true,
            "--print" => f.print = true,
            "--no-update" => f.no_update = true,
            "--force" => f.force = true,
            "--json" => f.json = true,
            "--non-interactive" => f.non_interactive = true,
            "-q" | "--quiet" => f.quiet = true,
            "-v" | "--verbose" => f.verbose = true,
            _ => pass.push(a.clone()),
        }
    }
    pass.extend(it.cloned());
    (f, pass)
}

/// Where root-w3bs-os may live besides PATH: ~/.local/bin (installed as a user) and
/// /usr/local/sbin (installed as root).
pub fn extra_dirs() -> Vec<PathBuf> {
    if let Some(v) = std::env::var_os(DIRS_ENV) {
        return std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect();
    }
    let mut d = vec![];
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        d.push(PathBuf::from(h).join(".local/bin"));
    }
    d.push(PathBuf::from("/usr/local/sbin"));
    d
}

fn is_exe(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    p.is_file()
}

/// `bin` in the PATH directories, then in `extra`.
pub fn find(bin: &str, path: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    let on_path: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    on_path.iter().chain(extra).map(|d| d.join(bin)).find(|p| is_exe(p))
}

/// Make the extra directories visible to everything this process runs (the engine's version
/// probe, the installer). Call before any thread starts.
pub fn extend_path() {
    let cur: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let add: Vec<PathBuf> = extra_dirs().into_iter().filter(|d| !cur.contains(d)).collect();
    if add.is_empty() {
        return;
    }
    if let Ok(joined) = std::env::join_paths(cur.into_iter().chain(add)) {
        // SAFETY: called from main before the runtime or any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// How root is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Privilege {
    Root,
    Via(PathBuf),
}

impl Privilege {
    pub fn name(&self) -> String {
        match self {
            Privilege::Root => "root".into(),
            Privilege::Via(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }
}

/// Root runs it directly; otherwise doas when present, else sudo, else exit 3.
pub fn privilege(root: bool, path: Option<&OsStr>) -> Result<Privilege> {
    if root {
        return Ok(Privilege::Root);
    }
    for b in ["doas", "sudo"] {
        if let Some(p) = find(b, path, &[]) {
            return Ok(Privilege::Via(p));
        }
    }
    Err(Error::approval("NEEDS_ROOT", "root-w3bs-os must run as root: run `w3bs os setup` as root, or install doas (W3BS OS: `apk add doas`) or sudo"))
}

/// The exact command: `[doas|sudo [-n]] <script> <args...>`.
pub fn command(p: &Privilege, non_interactive: bool, script: &Path, args: &[String]) -> Vec<String> {
    let mut c = vec![];
    if let Privilege::Via(b) = p {
        c.push(b.to_string_lossy().into_owned());
        if non_interactive {
            c.push("-n".into());
        }
    }
    c.push(script.to_string_lossy().into_owned());
    c.extend(args.iter().cloned());
    c
}

/// One shell-quoted line, for showing the command.
pub fn shell_line(words: &[String]) -> String {
    let safe = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=,:@+%".contains(&b));
    words.iter().map(|w| if safe(w) { w.clone() } else { format!("'{}'", w.replace('\'', "'\\''")) }).collect::<Vec<_>>().join(" ")
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

fn is_alpine() -> bool {
    Path::new("/etc/alpine-release").exists()
}

pub struct SetupOpts {
    pub flags: Flags,
    pub args: Vec<String>,
    /// --json: stdout carries the result, so the script's stdout goes to stderr.
    pub json: bool,
    pub non_interactive: bool,
}

fn outcome_ok(r: &OperationResult) -> bool {
    r.result == Overall::Ok
}

pub async fn setup(o: &SetupOpts, ui: &dyn Interact, mk: &dyn Fn(bool, bool) -> Engine) -> Result<OperationResult> {
    let f = &o.flags;
    let dry = f.dry_run || f.print;
    let mut res = OperationResult::new("os.setup");
    if !is_alpine() {
        if !f.force {
            return Err(Error::preflight(
                "NOT_ALPINE",
                "`w3bs os setup` sets up W3BS OS (Alpine Linux) boxes and this is not Alpine (no /etc/alpine-release). On Ubuntu use root-ubuntu.sh from profullstack/cli-tools; --force runs root-w3bs-os anyway (it refuses a non-Alpine system itself)",
            ));
        }
        res.warnings.push("not Alpine Linux: running anyway because of --force".into());
    }
    let extra = extra_dirs();
    let path = std::env::var_os("PATH");
    let found = find(SCRIPT, path.as_deref(), &extra);
    let step: Value;
    if found.is_none() {
        ui.progress(&format!("{SCRIPT} is not installed; installing {PACKAGE}"));
        let r = mk(f.yes, f.dry_run).install(InstallRequest { packages: vec![PACKAGE.into()], origin: Some("native".into()), ..Default::default() }).await?;
        res.packages.extend(r.packages.clone());
        step = json!({ "action": "install", "result": r.result });
        if !outcome_ok(&r) {
            res.result = r.result;
            res.error = r.error.or_else(|| r.packages.iter().find_map(|p| p.error.clone()));
            res.data = json!({ "package": PACKAGE, "install": step });
            return Ok(res);
        }
    } else if !f.no_update {
        // Managed by w3bs: update it. On the machine but not recorded (an image, a manual
        // install): install adopts it when current and runs the newer installer when not.
        let managed = mk(false, false).info(PACKAGE).is_ok();
        let r = if managed {
            mk(f.yes, f.dry_run).update(UpdateRequest { packages: vec![PACKAGE.into()], ..Default::default() }).await
        } else {
            mk(f.yes, f.dry_run).install(InstallRequest { packages: vec![PACKAGE.into()], origin: Some("native".into()), ..Default::default() }).await
        };
        match r {
            Ok(r) if outcome_ok(&r) => {
                res.packages.extend(r.packages);
                step = json!({ "action": if managed { "update" } else { "adopt" }, "result": "ok" });
            }
            Ok(r) if r.result == Overall::ApprovalRequired => {
                res.result = r.result;
                res.error = r.error.or_else(|| r.packages.iter().find_map(|p| p.error.clone()));
                res.packages.extend(r.packages);
                return Ok(res);
            }
            Ok(r) => {
                let why = r.error.map(|e| e.message).or_else(|| r.packages.iter().find_map(|p| p.error.as_ref().map(|e| e.message.clone()))).unwrap_or_else(|| "failed".into());
                res.warnings.push(format!("could not update {PACKAGE} ({why}); running the installed {SCRIPT}"));
                res.packages.extend(r.packages);
                step = json!({ "action": "update", "result": "failed" });
            }
            Err(e) => {
                res.warnings.push(format!("could not update {PACKAGE} ({}); running the installed {SCRIPT}", e.message));
                step = json!({ "action": "update", "result": "failed" });
            }
        }
    } else {
        step = json!({ "action": "none", "reason": "--no-update" });
    }
    let root = is_root();
    let script = match find(SCRIPT, path.as_deref(), &extra) {
        Some(s) => s,
        None if dry => if root { PathBuf::from("/usr/local/sbin").join(SCRIPT) } else { extra.first().cloned().unwrap_or_default().join(SCRIPT) },
        None => return Err(Error::package("SCRIPT_MISSING", format!("{PACKAGE} is installed but no `{SCRIPT}` was found on PATH, in ~/.local/bin or in /usr/local/sbin"))),
    };
    let how = privilege(root, path.as_deref())?;
    let cmd = command(&how, o.non_interactive && !o.json, &script, &o.args);
    let line = shell_line(&cmd);
    res.data = json!({
        "package": PACKAGE, "script": script, "update": step, "args": o.args, "privilege": how.name(),
        "command": cmd, "command_line": line, "dry_run": dry,
    });
    if dry {
        return Ok(res);
    }
    ui.progress(&format!("running {line}"));
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).stdin(Stdio::inherit()).stderr(Stdio::inherit());
    if o.json {
        c.stdout(Stdio::from(std::io::stderr()));
        let st = c.status().map_err(|e| Error::package("SETUP_FAILED", format!("could not run {}: {e}", cmd[0])))?;
        let code = st.code().unwrap_or(1);
        res.data["exit_code"] = code.into();
        if code != 0 {
            res.result = Overall::Failure;
            res.error = Some(Error::package("SETUP_FAILED", format!("{SCRIPT} exited with {code}")).info());
        }
        return Ok(res);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = c.exec();
        Err(Error::package("SETUP_FAILED", format!("could not run {}: {err}", cmd[0])))
    }
    #[cfg(not(unix))]
    {
        let st = c.status().map_err(|e| Error::package("SETUP_FAILED", format!("could not run {}: {e}", cmd[0])))?;
        std::process::exit(st.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn tail_after_os_setup() {
        assert_eq!(tail(&s(&["w3bs", "os", "setup", "--", "anthony"])), s(&["--", "anthony"]));
        assert_eq!(tail(&s(&["w3bs", "--json", "os", "--quiet", "setup", "a"])), s(&["a"]));
        assert_eq!(tail(&s(&["w3bs", "os", "setup"])), Vec::<String>::new());
        assert_eq!(tail(&s(&["w3bs", "os", "status"])), Vec::<String>::new());
        // A later `setup` (a user named setup) stays an argument.
        assert_eq!(tail(&s(&["w3bs", "os", "setup", "setup", "os"])), s(&["setup", "os"]));
    }

    #[test]
    fn split_flags_from_script_args() {
        let (f, a) = split_args(&s(&["--yes", "--", "anthony", "preshy", "--groups", "wheel"]));
        assert_eq!((f.yes, a), (true, s(&["anthony", "preshy", "--groups", "wheel"])));
        // Bare: w3bs flags anywhere before --, the rest in order.
        let (f, a) = split_args(&s(&["anthony", "--refresh", "-n", "--groups", "wheel,docker", "--no-update", "mount"]));
        assert!(f.dry_run && f.no_update && !f.yes);
        assert_eq!(a, s(&["anthony", "--refresh", "--groups", "wheel,docker", "mount"]));
        // After --, even w3bs's flags and another -- are the script's.
        let (f, a) = split_args(&s(&["--print", "--", "--yes", "--", "--json", "-n"]));
        assert_eq!(f, Flags { print: true, ..Default::default() });
        assert_eq!(a, s(&["--yes", "--", "--json", "-n"]));
        let (f, a) = split_args(&s(&["--json", "--force", "--with-sh1pt", "--without-moshcode"]));
        assert!(f.json && f.force);
        assert_eq!(a, s(&["--with-sh1pt", "--without-moshcode"]));
        assert_eq!(split_args(&[]), (Flags::default(), vec![]));
    }

    fn stub(dir: &Path, name: &str, log: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        // Record argv, then run the rest (as doas/sudo would).
        let body = if name == SCRIPT { format!("#!/bin/sh\necho \"{name} $*\" >> {}\nexit 0\n", log.display()) } else { format!("#!/bin/sh\necho \"{name} $*\" >> {}\n[ \"$1\" = -n ] && shift\nexec \"$@\"\n", log.display()) };
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run_and_log(cmd: &[String], log: &Path) -> Vec<String> {
        let st = Command::new(&cmd[0]).args(&cmd[1..]).env("PATH", "/usr/bin:/bin").status().unwrap();
        assert!(st.success());
        std::fs::read_to_string(log).unwrap().lines().map(str::to_owned).collect()
    }

    #[test]
    fn privilege_prefers_root_then_doas_then_sudo() {
        let t = tempfile::tempdir().unwrap();
        let (both, only_sudo, none, sbin) = (t.path().join("both"), t.path().join("sudo"), t.path().join("none"), t.path().join("sbin"));
        for d in [&both, &only_sudo, &none, &sbin] {
            std::fs::create_dir_all(d).unwrap();
        }
        let log = t.path().join("argv");
        stub(&both, "doas", &log);
        stub(&both, "sudo", &log);
        stub(&only_sudo, "sudo", &log);
        stub(&sbin, SCRIPT, &log);
        let path = |d: &[&Path]| std::env::join_paths(d).unwrap();
        let args = s(&["anthony", "preshy", "--groups", "wheel", "it's"]);

        // The script is found in an extra directory when PATH does not have it.
        let script = find(SCRIPT, Some(&path(&[&none])), std::slice::from_ref(&sbin)).unwrap();
        assert_eq!(script, sbin.join(SCRIPT));
        assert_eq!(find(SCRIPT, Some(&path(&[&none])), &[]), None);

        assert_eq!(privilege(true, Some(&path(&[&both]))).unwrap(), Privilege::Root);
        let root = command(&Privilege::Root, false, &script, &args);
        assert_eq!(run_and_log(&root, &log), vec![format!("{SCRIPT} anthony preshy --groups wheel it's")]);

        let p = privilege(false, Some(&path(&[&none, &both]))).unwrap();
        assert_eq!(p, Privilege::Via(both.join("doas")));
        std::fs::remove_file(&log).unwrap();
        assert_eq!(run_and_log(&command(&p, false, &script, &args), &log), vec![format!("doas {} anthony preshy --groups wheel it's", script.display()), format!("{SCRIPT} anthony preshy --groups wheel it's")]);

        let p = privilege(false, Some(&path(&[&only_sudo]))).unwrap();
        assert_eq!(p.name(), "sudo");
        std::fs::remove_file(&log).unwrap();
        let c = command(&p, true, &script, &args);
        assert_eq!(c[1], "-n");
        assert_eq!(run_and_log(&c, &log)[0], format!("sudo -n {} anthony preshy --groups wheel it's", script.display()));

        let e = privilege(false, Some(&path(&[&none]))).unwrap_err();
        assert_eq!((e.code, e.overall().exit_code()), ("NEEDS_ROOT", 3));
    }

    #[test]
    fn shell_line_quotes_only_what_needs_it() {
        assert_eq!(shell_line(&s(&["doas", "/usr/local/sbin/root-w3bs-os", "anthony", "--groups", "wheel,docker"])), "doas /usr/local/sbin/root-w3bs-os anthony --groups wheel,docker");
        assert_eq!(shell_line(&s(&["x", "a b", "it's", ""])), "x 'a b' 'it'\\''s' ''");
    }
}
