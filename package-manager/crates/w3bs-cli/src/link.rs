//! Install links (docs/os-prd.md §21): `w3bs://install/<namespace>/<name>[?channel=<c>]` and
//! `https://w3bs.org/i/<namespace>/<name>[?channel=<c>]`.
//!
//! A link comes from the web, so it is parsed strictly and never trusted: it may only name a
//! package identity and a channel, both validated, and `w3bs open` always shows the normal
//! install plan (with the publisher signature status) and asks before anything changes.
//! Nothing from the raw link is ever passed to a shell; a relaunch in a terminal carries the
//! canonical link rebuilt from the validated parts.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use w3bs_schema::identity::PackageId;
use w3bs_schema::manifest::validate_channel;

/// The longest link accepted; real links are far shorter.
const MAX_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: PackageId,
    pub channel: Option<String>,
}

impl Link {
    /// The canonical `w3bs://` form, rebuilt from validated parts only.
    pub fn canonical(&self) -> String {
        match &self.channel {
            Some(c) => format!("w3bs://install/{}?channel={c}", self.id),
            None => format!("w3bs://install/{}", self.id),
        }
    }
}

/// Parse an install link. Every refusal says why, so a person who clicked it knows what to do.
pub fn parse(raw: &str) -> Result<Link, String> {
    if raw.len() > MAX_LEN {
        return Err(format!("install link is longer than {MAX_LEN} characters"));
    }
    if let Some(c) = raw.chars().find(|c| !c.is_ascii_graphic()) {
        return Err(format!("install link contains a character that is not allowed ({c:?})"));
    }
    if raw.contains('%') {
        return Err("install link must not be percent-encoded".into());
    }
    if raw.contains('#') {
        return Err("install link must not have a #fragment".into());
    }
    if raw.contains('\\') {
        return Err("install link must not contain a backslash".into());
    }
    let rest = if let Some(r) = raw.strip_prefix("w3bs://") {
        match r.split_once('/') {
            Some(("install", path)) => path,
            _ if r == "os" || r.starts_with("os/") || r.starts_with("os?") => {
                return Err("W3BS OS links are not handled by the CLI yet; open https://w3bs.org/i/os in a browser".into());
            }
            _ => {
                return Err(format!(
                    "`{raw}` is not an install link (w3bs://install/<namespace>/<name>); for w3bs:// resources use `w3bs resolve`"
                ));
            }
        }
    } else if let Some(r) = raw.strip_prefix("https://w3bs.org/i/") {
        r
    } else {
        return Err(format!(
            "`{raw}` is not an install link; expected w3bs://install/<namespace>/<name> or https://w3bs.org/i/<namespace>/<name> (scheme and host in lowercase)"
        ));
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    // Browsers and handlers sometimes add a single trailing slash; nothing more.
    let path = path.strip_suffix('/').unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() != 2 {
        return Err(format!("install link must name exactly <namespace>/<name>, got `{path}`"));
    }
    if segments.iter().any(|s| s.is_empty() || *s == "." || *s == "..") {
        return Err(format!("install link has an empty or relative path segment: `{path}`"));
    }
    let id: PackageId = path.parse().map_err(|e| format!("install link names an invalid package: {e}"))?;
    let channel = match query {
        None => None,
        Some(q) => {
            let (k, v) = q.split_once('=').ok_or_else(|| format!("install link query `{q}` must be channel=<name>"))?;
            if k != "channel" {
                return Err(format!("install link may only carry `channel`, not `{k}`"));
            }
            validate_channel(v).map_err(|e| format!("install link: {e}"))?;
            Some(v.to_owned())
        }
    };
    Ok(Link { id, channel })
}

/// Whether standard input is a terminal (a person can answer the confirmation).
pub fn stdin_is_tty() -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(0) == 1 }
}

/// Find an executable on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_exec(&p).then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| is_exec(p))
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The shell program run inside the terminal. The binary and the canonical link arrive as
/// positional parameters ($0, $1), never spliced into the script text.
const KEEP_OPEN: &str = r#""$0" open --no-terminal "$1"; s=$?; printf '\n[w3bs finished with exit code %s] Press Enter to close. ' "$s"; read -r _ || true"#;

/// Terminal emulators tried in order, with the argument that introduces the command
/// (empty: the command follows directly).
const LINUX_TERMINALS: &[(&str, &[&str])] = &[
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("kgx", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("alacritty", &["-e"]),
    ("kitty", &[]),
    ("foot", &[]),
    ("xterm", &["-e"]),
];

/// Whether there is a desktop session a terminal window could open on.
pub fn has_desktop() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()))
}

/// Re-run `w3bs open` for `link` inside a new terminal window. Returns the terminal used.
pub fn relaunch_in_terminal(exe: &Path, link: &Link) -> Result<String, String> {
    let url = link.canonical();
    if cfg!(target_os = "macos") {
        return relaunch_macos(exe, &url);
    }
    let mut candidates: Vec<(String, PathBuf, Vec<&str>)> = vec![];
    if let Some(p) = which("xdg-terminal-exec") {
        candidates.push(("xdg-terminal-exec".into(), p, vec![]));
    }
    if let Some(t) = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty() && !t.contains(char::is_whitespace))
        && let Some(p) = which(&t)
    {
        candidates.push((t, p, vec!["-e"]));
    }
    for (name, flag) in LINUX_TERMINALS {
        if let Some(p) = which(name) {
            candidates.push(((*name).into(), p, flag.to_vec()));
        }
    }
    for (name, prog, flag) in candidates {
        let mut c = Command::new(&prog);
        c.args(&flag).arg("sh").arg("-c").arg(KEEP_OPEN).arg(exe).arg(&url);
        if spawn_detached(&mut c).is_ok() {
            return Ok(name);
        }
    }
    Err("no terminal emulator found (tried xdg-terminal-exec, $TERMINAL, konsole, gnome-terminal, kgx, xfce4-terminal, alacritty, kitty, foot, xterm)".into())
}

fn relaunch_macos(exe: &Path, url: &str) -> Result<String, String> {
    // AppleScript's `quoted form of` shell-quotes the arguments; they arrive through argv.
    let script = [
        "on run argv",
        "set cmd to quoted form of (item 1 of argv) & \" open --no-terminal \" & quoted form of (item 2 of argv) & \"; s=$?; printf '\\n[w3bs finished with exit code %s] Press Enter to close. ' \\\"$s\\\"; read _; exit\"",
        "tell application \"Terminal\"",
        "activate",
        "do script cmd",
        "end tell",
        "end run",
    ];
    let mut c = Command::new("osascript");
    for line in script {
        c.arg("-e").arg(line);
    }
    c.arg(exe).arg(url);
    let st = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e| format!("osascript: {e}"))?;
    if st.success() { Ok("Terminal".into()) } else { Err(format!("osascript could not open Terminal ({st})")) }
}

/// Start a terminal and let it run on its own. One that fails at once (no display, a broken
/// install) counts as a failure so the next candidate is tried; a launcher that hands the
/// window to a server and exits 0 (gnome-terminal) counts as success.
fn spawn_detached(c: &mut Command) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    let mut child = c.spawn()?;
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(1500) {
        match child.try_wait()? {
            Some(st) if !st.success() => return Err(std::io::Error::other(format!("exited with {st}"))),
            Some(_) => return Ok(()),
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Link {
        parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }
    fn bad(s: &str) -> String {
        parse(s).expect_err(s)
    }

    #[test]
    fn accepts_install_links() {
        let l = ok("w3bs://install/pairux.com/pairux");
        assert_eq!(l.id.to_string(), "pairux.com/pairux");
        assert_eq!(l.channel, None);
        assert_eq!(l.canonical(), "w3bs://install/pairux.com/pairux");
        let l = ok("w3bs://install/w3bs.org/w3bs?channel=beta");
        assert_eq!(l.channel.as_deref(), Some("beta"));
        assert_eq!(l.canonical(), "w3bs://install/w3bs.org/w3bs?channel=beta");
        assert_eq!(ok("https://w3bs.org/i/pairux.com/pairux").id.to_string(), "pairux.com/pairux");
        assert_eq!(ok("https://w3bs.org/i/pairux.com/pairux?channel=nightly-2").channel.as_deref(), Some("nightly-2"));
        assert_eq!(ok("w3bs://install/pairux.com/pairux/").id.to_string(), "pairux.com/pairux");
    }

    #[test]
    fn refuses_traversal_and_shape() {
        for s in [
            "w3bs://install/../x",
            "w3bs://install/./x",
            "w3bs://install/a.com/..",
            "w3bs://install/a.com//x",
            "w3bs://install/a.com/x/extra",
            "w3bs://install/a.com/x//",
            "w3bs://install/a.com",
            "w3bs://install/",
            "w3bs://install",
            "https://w3bs.org/i/a.com/x/y",
            "https://w3bs.org/i/../../etc/passwd",
        ] {
            bad(s);
        }
    }

    #[test]
    fn refuses_other_schemes_hosts_and_case() {
        for s in [
            "W3BS://install/a.com/x",
            "w3bs://INSTALL/a.com/x",
            "W3bs://install/a.com/x",
            "HTTPS://w3bs.org/i/a.com/x",
            "https://W3BS.org/i/a.com/x",
            "http://w3bs.org/i/a.com/x",
            "https://evil.example/i/a.com/x",
            "https://w3bs.org.evil.example/i/a.com/x",
            "https://w3bs.org@evil.example/i/a.com/x",
            "https://w3bs.org/install/a.com/x",
            "w3bs://prompt/a.com/x@1.0.0",
            "w3bs://os",
            "file:///etc/passwd",
            "",
            "a.com/x",
        ] {
            bad(s);
        }
        assert!(bad("w3bs://prompt/a.com/x@1.0.0").contains("w3bs resolve"));
        assert!(bad("w3bs://os").contains("w3bs.org/i/os"));
    }

    #[test]
    fn refuses_bad_identities() {
        for s in ["w3bs://install/A.com/x", "w3bs://install/a.com/X", "w3bs://install/a_b/x", "w3bs://install/a.com/-x", "w3bs://install/a.com/x@1.0.0"] {
            bad(s);
        }
    }

    #[test]
    fn refuses_query_injection_and_encoding() {
        for s in [
            "w3bs://install/a.com/x?channel=beta&yes=1",
            "w3bs://install/a.com/x?yes=1",
            "w3bs://install/a.com/x?channel=beta;rm -rf ~",
            "w3bs://install/a.com/x?channel=$(id)",
            "w3bs://install/a.com/x?channel=`id`",
            "w3bs://install/a.com/x?channel=Beta",
            "w3bs://install/a.com/x?channel=",
            "w3bs://install/a.com/x?channel",
            "w3bs://install/a.com/x?channel=a?channel=b",
            "w3bs://install/a.com/x?channel=beta#frag",
            "w3bs://install/a.com/x%2F..",
            "w3bs://install/a.com/x\n--yes",
            "w3bs://install/a.com/x --yes",
            "w3bs://install/a.com/x\\..",
            "w3bs://install/a.com/x'\"",
        ] {
            bad(s);
        }
        let long = format!("w3bs://install/a.com/{}", "x".repeat(600));
        bad(&long);
    }
}
