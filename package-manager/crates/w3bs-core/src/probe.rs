//! What version of a curl-installed tool is on this machine. W3BS does not own those files,
//! so the only witness is the tool itself: run its first command with `--version` (stdin
//! closed, a few seconds at most) and take the first `x.y.z` it prints.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

/// The first command's version, or None when it is missing, hangs, fails or prints no version.
pub fn installed_version(commands: &[String], bin_dir: &Path) -> Option<String> {
    let cmd = commands.first()?;
    if cmd.is_empty() || cmd.contains('/') {
        return None;
    }
    let exe = find(cmd, bin_dir)?;
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("W3BS", "1")
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut out);
    }
    parse_version(&out)
}

/// Probe several tools at once; `list` and `outdated` would otherwise wait on each in turn.
pub fn installed_versions(all: &[Vec<String>], bin_dir: &Path) -> Vec<Option<String>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = all.iter().map(|commands| scope.spawn(move || installed_version(commands, bin_dir))).collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    })
}

/// True only when both versions are known and the installed one is not older than the
/// catalog's `latest_version`; then the publisher's installer has nothing to do.
pub fn is_current(installed: Option<&str>, latest: Option<&str>) -> bool {
    let v = |s: &str| semver::Version::parse(s.trim().trim_start_matches(['v', 'V'])).ok();
    matches!((installed.and_then(v), latest.and_then(v)), (Some(i), Some(l)) if i >= l)
}

/// W3BS's own bin dir first (where installers usually put shims), then PATH.
fn find(cmd: &str, bin_dir: &Path) -> Option<PathBuf> {
    let own = bin_dir.join(cmd);
    if own.is_file() {
        return Some(own);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(cmd)).find(|p| p.is_file())
}

/// The first `MAJOR.MINOR.PATCH` (with an optional `-pre`/`+build` tail) in `text`.
pub fn parse_version(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // A version starts at a digit that does not continue a longer word or number;
        // a lone `v` in front (v1.2.3) is allowed.
        let word = |k: usize| b[k].is_ascii_alphanumeric() || b[k] == b'.';
        let v_prefix = i >= 1 && (b[i - 1] == b'v' || b[i - 1] == b'V') && (i == 1 || !word(i - 2));
        if b[i].is_ascii_digit() && (i == 0 || !word(i - 1) || v_prefix) {
            let mut j = i;
            let mut parts = 0;
            loop {
                let start = j;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j == start {
                    break;
                }
                parts += 1;
                if parts == 3 || j >= b.len() || b[j] != b'.' {
                    break;
                }
                j += 1;
            }
            // A fourth numeric part means an address like 10.0.0.1, not a version.
            let fourth = j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit();
            if parts == 3 && !fourth {
                let mut end = j;
                if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
                    let mut k = end + 1;
                    while k < b.len() && (b[k].is_ascii_alphanumeric() || b"-+.".contains(&b[k])) {
                        k += 1;
                    }
                    if k > end + 1 {
                        end = k;
                    }
                }
                return Some(text[i..end].trim_end_matches(['.', '-', '+']).to_owned());
            }
            // Skip the rest of this number so its tail is not read as a fresh start.
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn finds_the_first_semver() {
        assert_eq!(parse_version("0.14.1\n").as_deref(), Some("0.14.1"));
        assert_eq!(parse_version("w3bs 0.1.9").as_deref(), Some("0.1.9"));
        assert_eq!(parse_version("pairux v0.17.0 (linux)").as_deref(), Some("0.17.0"));
        assert_eq!(parse_version("tool 2.0.0-rc.1+abc built").as_deref(), Some("2.0.0-rc.1+abc"));
        assert_eq!(parse_version("ends with a dot 1.2.3.").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn ignores_what_is_not_a_version() {
        assert_eq!(parse_version("node 22 on linux"), None);
        assert_eq!(parse_version("ip 10.0.0.1 is not it"), None);
        assert_eq!(parse_version("listening on 10.0.0.1, version 3.4.5").as_deref(), Some("3.4.5"));
        assert_eq!(parse_version("sha abc1.2.3"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn current_needs_both_versions() {
        use super::is_current;
        assert!(is_current(Some("0.16.2"), Some("0.12.5")));
        assert!(is_current(Some("1.0.0"), Some("v1.0.0")));
        assert!(!is_current(Some("1.0.0"), Some("1.0.1")));
        assert!(!is_current(Some("1.0.0-rc.1"), Some("1.0.0")));
        assert!(!is_current(None, Some("1.0.0")));
        assert!(!is_current(Some("1.0.0"), None));
    }
}
