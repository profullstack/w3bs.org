//! `w3bs dist-upgrade`: upgrade the base system through its own package manager (apk, apt, dnf,
//! pacman, zypper or brew), then every W3BS package, the w3bs CLI included. This stays within
//! the installed release; moving a W3BS OS machine to a new release is `w3bs os upgrade`.

use serde_json::{Value, json};
use std::process::{Command, Stdio};
use w3bs_core::{Error, Result};

#[derive(Debug, PartialEq)]
pub struct System {
    pub provider: &'static str,
    /// Commands run in order, each under sudo or doas when `root` and not already root.
    pub steps: Vec<Vec<&'static str>>,
    pub root: bool,
}

/// The system package manager, from os-release `ID` and `ID_LIKE` (brew on macOS).
pub fn detect(os_release: &str, macos: bool) -> Option<System> {
    if macos {
        return Some(System { provider: "brew", steps: vec![vec!["brew", "update"], vec!["brew", "upgrade"]], root: false });
    }
    let field = |k: &str| os_release.lines().find_map(|l| l.strip_prefix(k).and_then(|v| v.strip_prefix('='))).map(|v| v.trim().trim_matches('"').to_lowercase()).unwrap_or_default();
    let ids = format!("{} {}", field("ID"), field("ID_LIKE"));
    let is = |n: &str| ids.split_whitespace().any(|i| i == n);
    let (provider, steps): (&'static str, Vec<Vec<&'static str>>) = if is("alpine") {
        ("apk", vec![vec!["apk", "update"], vec!["apk", "upgrade", "--available"]])
    } else if is("debian") || is("ubuntu") {
        ("apt", vec![vec!["apt-get", "update"], vec!["apt-get", "-y", "dist-upgrade"]])
    } else if is("fedora") || is("rhel") || is("centos") {
        ("dnf", vec![vec!["dnf", "-y", "distro-sync", "--refresh"]])
    } else if is("arch") {
        ("pacman", vec![vec!["pacman", "-Syu", "--noconfirm"]])
    } else if is("suse") || is("opensuse") {
        ("zypper", vec![vec!["zypper", "--non-interactive", "dist-upgrade"]])
    } else {
        return None;
    };
    Some(System { provider, steps, root: true })
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
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

use w3bs_core::engine::in_container;

const CONTAINER_NOTE: &str = "in a container the base system comes from its image: pull the newer image (for W3BS OS: docker pull w3bs.org/os:server) and recreate the container";

/// Upgrade the base system, after one approval (`--yes` skips it; `--dry-run` only plans).
pub fn system(yes: bool, dry_run: bool, confirm: impl Fn(&str) -> Option<bool>, progress: impl Fn(&str)) -> Result<Value> {
    if std::env::var_os("W3BS_HANDOFF").is_some() {
        return Ok(json!({ "status": "done", "note": "upgraded before w3bs updated itself" }));
    }
    let os_release = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default();
    let container = in_container();
    let Some(sys) = detect(&os_release, cfg!(target_os = "macos")) else {
        return Ok(json!({ "status": "skipped", "note": "no supported system package manager here; only W3BS packages are upgraded" }));
    };
    let prefix = if !sys.root || is_root() {
        None
    } else if on_path("sudo") {
        Some("sudo")
    } else if on_path("doas") {
        Some("doas")
    } else {
        let note = if container { CONTAINER_NOTE.to_owned() } else { format!("{} needs root and neither sudo nor doas is installed: run `w3bs dist-upgrade` as root", sys.provider) };
        return Ok(json!({ "provider": sys.provider, "status": "skipped", "note": note }));
    };
    let argv: Vec<Vec<&str>> = sys.steps.iter().map(|s| prefix.into_iter().chain(s.iter().copied()).collect()).collect();
    let commands: Vec<String> = argv.iter().map(|a| a.join(" ")).collect();
    let note = container.then_some(CONTAINER_NOTE);
    if dry_run {
        return Ok(json!({ "provider": sys.provider, "commands": commands, "status": "planned", "note": note }));
    }
    if !yes {
        match confirm(&format!("W3BS will upgrade the system ({}), then every W3BS package:\n  {}\nProceed?", sys.provider, commands.join("\n  "))) {
            Some(true) => {}
            Some(false) => return Err(Error::approval("DECLINED", "declined; nothing was changed")),
            None => return Err(Error::approval("APPROVAL_REQUIRED", "no terminal to ask for approval: re-run with --yes")),
        }
    }
    for (cmd, line) in argv.iter().zip(&commands) {
        progress(&format!("running {line}"));
        // The system tool's own output goes to stderr so `--json` stays parseable.
        let status = Command::new(cmd[0]).args(&cmd[1..]).stdin(Stdio::inherit()).stdout(std::io::stderr()).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => return Err(Error::package("SYSTEM_UPGRADE_FAILED", format!("`{line}` failed ({s}); W3BS packages were not upgraded"))),
            Err(e) => return Err(Error::package("SYSTEM_UPGRADE_FAILED", format!("could not run `{line}`: {e}"))),
        }
    }
    Ok(json!({ "provider": sys.provider, "commands": commands, "status": "upgraded", "note": note }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_system_package_manager() {
        let p = |s: &str| detect(s, false).map(|x| x.provider);
        assert_eq!(p("NAME=\"Alpine Linux\"\nID=alpine\nVERSION_ID=3.24.2"), Some("apk"));
        assert_eq!(p("ID=postmarketos\nID_LIKE=alpine"), Some("apk"));
        assert_eq!(p("ID=ubuntu\nID_LIKE=debian"), Some("apt"));
        assert_eq!(p("ID=linuxmint\nID_LIKE=\"ubuntu debian\""), Some("apt"));
        assert_eq!(p("ID=fedora"), Some("dnf"));
        assert_eq!(p("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\""), Some("dnf"));
        assert_eq!(p("ID=endeavouros\nID_LIKE=arch"), Some("pacman"));
        assert_eq!(p("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\""), Some("zypper"));
        assert_eq!(p("ID=nixos"), None);
        assert_eq!(p(""), None);
        // IDENTIFIER-like keys must not match ID.
        assert_eq!(p("ID_LIKEX=alpine\nID=nixos"), None);
        assert_eq!(detect("", true).map(|x| (x.provider, x.root)), Some(("brew", false)));
    }

    #[test]
    fn alpine_upgrades_to_the_available_versions() {
        assert_eq!(detect("ID=alpine", false).unwrap().steps, vec![vec!["apk", "update"], vec!["apk", "upgrade", "--available"]]);
    }
}
