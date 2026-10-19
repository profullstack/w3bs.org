#![cfg_attr(target_os = "macos", allow(dead_code))]
//! `w3bs handler install|uninstall|status`: register `w3bs open` as this user's handler for
//! `w3bs://` links. Opt-in only; nothing registers it silently.
//!
//! Linux: a hidden desktop entry in `$XDG_DATA_HOME/applications` plus `xdg-mime default`.
//! macOS: a tiny AppleScript applet in ~/Applications (macOS delivers URLs to apps as Apple
//! Events, not argv, so a plain shell stub would never see the link), registered with
//! Launch Services.

use crate::link::which;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use w3bs_core::{Error, Layout, Result};

pub const DESKTOP_FILE: &str = "w3bs-url-handler.desktop";
const MIME: &str = "x-scheme-handler/w3bs";
#[cfg(target_os = "macos")]
const APP_NAME: &str = "W3BS URL Handler.app";
#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "org.w3bs.url-handler";
#[cfg(target_os = "macos")]
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute()).ok_or_else(|| Error::preflight("NO_HOME", "HOME is not set to an absolute path"))
}

fn xdg(var: &str, default: &str) -> Result<PathBuf> {
    match std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(p) => Ok(p),
        None => Ok(home()?.join(default)),
    }
}

/// The `w3bs` the handler should run: the stable command in the bin directory when it exists
/// (it survives updates), otherwise this binary.
fn w3bs_path() -> Result<PathBuf> {
    if let Ok(l) = Layout::from_env() {
        let p = l.bin_dir.join("w3bs");
        if p.is_file() {
            return Ok(p);
        }
    }
    std::env::current_exe().map_err(|e| Error::preflight("NO_EXE", format!("cannot find the w3bs binary: {e}")))
}

fn io(what: &str, p: &Path, e: std::io::Error) -> Error {
    Error::preflight("HANDLER_IO", format!("{what} {}: {e}", p.display()))
}

/// Run a helper, capturing stdout; `None` when it is not installed.
fn run(prog: &str, args: &[&str]) -> Option<std::result::Result<String, String>> {
    let p = which(prog)?;
    Some(match Command::new(p).args(args).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        Ok(o) => Err(format!("{prog} {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("{prog}: {e}")),
    })
}

// ------------------------------------------------------------------ Linux

/// Quote one argument for a desktop entry Exec key: the Exec quoting rules, then the
/// string-value escaping of backslashes, and `%` doubled so it is not a field code.
pub fn exec_arg(s: &str) -> String {
    let safe = !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._+-,:@".contains(&b));
    let quoted = if safe {
        s.to_owned()
    } else {
        let mut q = String::from("\"");
        for c in s.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    };
    quoted.replace('\\', "\\\\").replace('%', "%%")
}

pub fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=W3BS install link\nComment=Opens w3bs:// install links in a terminal and asks before installing\nExec={} open %u\nTerminal=false\nNoDisplay=true\nMimeType={MIME};\n",
        exec_arg(&exe.to_string_lossy())
    )
}

fn apps_dir() -> Result<PathBuf> {
    Ok(xdg("XDG_DATA_HOME", ".local/share")?.join("applications"))
}

fn query_default() -> Value {
    match run("xdg-mime", &["query", "default", MIME]) {
        Some(Ok(s)) if !s.is_empty() => json!(s),
        _ => Value::Null,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() -> Result<Value> {
    let exe = w3bs_path()?;
    let dir = apps_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| io("cannot create", &dir, e))?;
    let file = dir.join(DESKTOP_FILE);
    let tmp = dir.join(format!(".{DESKTOP_FILE}.tmp"));
    std::fs::write(&tmp, desktop_entry(&exe)).map_err(|e| io("cannot write", &tmp, e))?;
    std::fs::rename(&tmp, &file).map_err(|e| io("cannot write", &file, e))?;
    let mut warnings = vec![];
    match run("xdg-mime", &["default", DESKTOP_FILE, MIME]) {
        Some(Ok(_)) => {}
        Some(Err(e)) => warnings.push(e),
        None => warnings.push("xdg-mime is not installed (package xdg-utils); the desktop entry is written but may not be the default".into()),
    }
    let dir_s = dir.to_string_lossy().into_owned();
    if let Some(Err(e)) = run("update-desktop-database", &[&dir_s]) {
        warnings.push(e);
    }
    let default = query_default();
    Ok(json!({
        "platform": "linux",
        "installed": true,
        "desktop_file": file,
        "exec": exe,
        "default": default,
        "registered": default == DESKTOP_FILE,
        "warnings": warnings,
    }))
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall() -> Result<Value> {
    let dir = apps_dir()?;
    let file = dir.join(DESKTOP_FILE);
    let removed = match std::fs::remove_file(&file) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(io("cannot remove", &file, e)),
    };
    let mimeapps = xdg("XDG_CONFIG_HOME", ".config")?.join("mimeapps.list");
    let unset = match std::fs::read_to_string(&mimeapps) {
        Ok(text) => {
            let (new, changed) = drop_association(&text);
            if changed {
                std::fs::write(&mimeapps, new).map_err(|e| io("cannot write", &mimeapps, e))?;
            }
            changed
        }
        Err(_) => false,
    };
    let dir_s = dir.to_string_lossy().into_owned();
    let _ = run("update-desktop-database", &[&dir_s]);
    Ok(json!({ "platform": "linux", "installed": false, "removed": removed, "desktop_file": file, "association_removed": unset }))
}

/// Remove our desktop file from every `x-scheme-handler/w3bs=` line of a mimeapps.list.
pub fn drop_association(text: &str) -> (String, bool) {
    let mut changed = false;
    let mut out = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix(&format!("{MIME}=")) {
            let keep: Vec<&str> = v.split(';').filter(|x| !x.is_empty() && *x != DESKTOP_FILE).collect();
            if keep.len() != v.split(';').filter(|x| !x.is_empty()).count() {
                changed = true;
                if !keep.is_empty() {
                    out.push_str(&format!("{MIME}={};\n", keep.join(";")));
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, changed)
}

#[cfg(not(target_os = "macos"))]
pub fn status() -> Result<Value> {
    let file = apps_dir()?.join(DESKTOP_FILE);
    let exec = std::fs::read_to_string(&file).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("Exec=").map(str::to_owned)));
    let default = query_default();
    Ok(json!({
        "platform": "linux",
        "installed": file.is_file(),
        "desktop_file": file,
        "exec": exec,
        "default": default,
        "registered": default == DESKTOP_FILE,
    }))
}

// ------------------------------------------------------------------ macOS (untested)

#[cfg(target_os = "macos")]
fn app_dir() -> Result<PathBuf> {
    Ok(home()?.join("Applications").join(APP_NAME))
}

/// An AppleScript string literal.
#[cfg(target_os = "macos")]
fn as_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<Value> {
    let exe = w3bs_path()?;
    let app = app_dir()?;
    let parent = app.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| io("cannot create", &parent, e))?;
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| io("cannot replace", &app, e))?;
    }
    // `open location` receives the URL; `w3bs open` validates it and opens Terminal itself.
    let source = format!(
        "on open location theURL\n  do shell script quoted form of {} & \" open \" & quoted form of theURL & \" >/dev/null 2>&1 &\"\nend open location\non run\n  display dialog \"W3BS URL Handler opens w3bs:// install links. Remove it with: w3bs handler uninstall\" buttons {{\"OK\"}} default button 1\nend run\n",
        as_string(&exe.to_string_lossy())
    );
    let tmp = std::env::temp_dir().join(format!("w3bs-url-handler-{}.applescript", std::process::id()));
    std::fs::write(&tmp, source).map_err(|e| io("cannot write", &tmp, e))?;
    let app_s = app.to_string_lossy().into_owned();
    let tmp_s = tmp.to_string_lossy().into_owned();
    let r = run("osacompile", &["-o", &app_s, &tmp_s]);
    let _ = std::fs::remove_file(&tmp);
    match r {
        Some(Ok(_)) => {}
        Some(Err(e)) => return Err(Error::preflight("HANDLER_BUILD_FAILED", e)),
        None => return Err(Error::preflight("HANDLER_BUILD_FAILED", "osacompile not found")),
    }
    let plist = app.join("Contents/Info.plist");
    let plist_s = plist.to_string_lossy().into_owned();
    let pb = "/usr/libexec/PlistBuddy";
    let _ = Command::new(pb).args(["-c", "Delete :CFBundleIdentifier", &plist_s]).output();
    let _ = Command::new(pb).args(["-c", "Delete :CFBundleURLTypes", &plist_s]).output();
    for cmd in [
        format!("Add :CFBundleIdentifier string {BUNDLE_ID}"),
        "Add :LSUIElement bool true".to_owned(),
        "Add :CFBundleURLTypes array".to_owned(),
        "Add :CFBundleURLTypes:0 dict".to_owned(),
        "Add :CFBundleURLTypes:0:CFBundleURLName string W3BS install link".to_owned(),
        "Add :CFBundleURLTypes:0:CFBundleURLSchemes array".to_owned(),
        "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string w3bs".to_owned(),
    ] {
        let o = Command::new(pb).args(["-c", &cmd, &plist_s]).output().map_err(|e| Error::preflight("HANDLER_BUILD_FAILED", format!("PlistBuddy: {e}")))?;
        if !o.status.success() {
            return Err(Error::preflight("HANDLER_BUILD_FAILED", format!("PlistBuddy {cmd}: {}", String::from_utf8_lossy(&o.stderr).trim())));
        }
    }
    let mut warnings = vec![];
    match Command::new(LSREGISTER).args(["-f", &app_s]).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => warnings.push(format!("lsregister: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => warnings.push(format!("lsregister: {e}")),
    }
    let default = mac_default();
    Ok(json!({ "platform": "macos", "installed": true, "app": app, "exec": exe, "default": default, "registered": default.as_str() == Some(app_s.as_str()), "warnings": warnings }))
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<Value> {
    let app = app_dir()?;
    let app_s = app.to_string_lossy().into_owned();
    let existed = app.exists();
    if existed {
        let _ = Command::new(LSREGISTER).args(["-u", &app_s]).output();
        std::fs::remove_dir_all(&app).map_err(|e| io("cannot remove", &app, e))?;
    }
    Ok(json!({ "platform": "macos", "installed": false, "removed": existed, "app": app }))
}

#[cfg(target_os = "macos")]
pub fn status() -> Result<Value> {
    let app = app_dir()?;
    let default = mac_default();
    let app_s = app.to_string_lossy().into_owned();
    Ok(json!({ "platform": "macos", "installed": app.is_dir(), "app": app, "default": default, "registered": default.as_str() == Some(app_s.as_str()) }))
}

/// The app Launch Services would open a w3bs:// link with.
#[cfg(target_os = "macos")]
fn mac_default() -> Value {
    let js = "ObjC.import('AppKit'); var u = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL($.NSURL.URLWithString('w3bs://install/w3bs.org/w3bs')); u.isNil() ? '' : ObjC.unwrap(u.path)";
    match run("osascript", &["-l", "JavaScript", "-e", js]) {
        Some(Ok(s)) if !s.is_empty() => json!(s),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting() {
        assert_eq!(exec_arg("/home/a/.local/bin/w3bs"), "/home/a/.local/bin/w3bs");
        assert_eq!(exec_arg("/home/a b/w3bs"), "\"/home/a b/w3bs\"");
        assert_eq!(exec_arg("/x/$HOME/w3bs"), "\"/x/\\\\$HOME/w3bs\"");
        assert_eq!(exec_arg("/x/100%/w3bs"), "\"/x/100%%/w3bs\"");
        let e = desktop_entry(Path::new("/opt/w3bs/bin/w3bs"));
        assert!(e.contains("Exec=/opt/w3bs/bin/w3bs open %u\n"));
        assert!(e.contains("MimeType=x-scheme-handler/w3bs;\n"));
        assert!(e.contains("NoDisplay=true\n"));
    }

    #[test]
    fn drops_only_our_association() {
        let t = "[Default Applications]\nx-scheme-handler/w3bs=w3bs-url-handler.desktop\ntext/html=firefox.desktop\n[Added Associations]\nx-scheme-handler/w3bs=other.desktop;w3bs-url-handler.desktop;\n";
        let (n, c) = drop_association(t);
        assert!(c);
        assert_eq!(n, "[Default Applications]\ntext/html=firefox.desktop\n[Added Associations]\nx-scheme-handler/w3bs=other.desktop;\n");
        assert!(!drop_association("text/html=firefox.desktop\n").1);
    }
}
