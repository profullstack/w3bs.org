//! `w3bs desktop setup gnome-extension`: install the bundled GNOME Shell extension (top-bar
//! native control + window data for the broker) for this user and turn it on.
//!
//! The files are compiled into this binary, written to
//! `$XDG_DATA_HOME/gnome-shell/extensions/agentic-desktop@w3bs.org/`, and the uuid is added to
//! `org.gnome.shell enabled-extensions` (what `gnome-extensions enable` and the Extensions app
//! change). GNOME Shell 45+ discovers newly installed extensions only when it starts, so on
//! Wayland the extension loads at the next login.

use adl_linux::gnome::EXTENSION_UUID;
use adl_schema::*;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

const FILES: &[(&str, &str)] = &[
    ("metadata.json", include_str!("../../../gnome-extension/agentic-desktop@w3bs.org/metadata.json")),
    ("extension.js", include_str!("../../../gnome-extension/agentic-desktop@w3bs.org/extension.js")),
    ("stylesheet.css", include_str!("../../../gnome-extension/agentic-desktop@w3bs.org/stylesheet.css")),
];

fn extensions_dir() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| {
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")).join(".local/share")
    });
    data.join("gnome-shell/extensions")
}

/// The strings in a GVariant `as` as `gsettings get` prints it (`@as []`, `['a', 'b']`).
fn parse_strv(s: &str) -> Vec<String> {
    s.split('\'').skip(1).step_by(2).map(str::to_owned).collect()
}

fn format_strv(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "))
}

fn gsettings(args: &[&str]) -> std::result::Result<String, String> {
    let out = Command::new("gsettings").args(args).output().map_err(|e| format!("gsettings: {e}"))?;
    if !out.status.success() {
        return Err(format!("gsettings {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Add the uuid to enabled-extensions and drop it from disabled-extensions.
fn enable() -> std::result::Result<Vec<String>, String> {
    let mut notes = vec![];
    let mut on = parse_strv(&gsettings(&["get", "org.gnome.shell", "enabled-extensions"])?);
    if !on.iter().any(|u| u == EXTENSION_UUID) {
        on.push(EXTENSION_UUID.into());
        gsettings(&["set", "org.gnome.shell", "enabled-extensions", &format_strv(&on)])?;
    }
    if let Ok(off) = gsettings(&["get", "org.gnome.shell", "disabled-extensions"]) {
        let off = parse_strv(&off);
        if off.iter().any(|u| u == EXTENSION_UUID) {
            let rest: Vec<String> = off.into_iter().filter(|u| u != EXTENSION_UUID).collect();
            gsettings(&["set", "org.gnome.shell", "disabled-extensions", &format_strv(&rest)])?;
        }
    }
    if gsettings(&["get", "org.gnome.shell", "disable-user-extensions"]).is_ok_and(|v| v == "true") {
        notes.push("User extensions are switched off globally (org.gnome.shell disable-user-extensions); turn them on in the Extensions app.".into());
    }
    Ok(notes)
}

pub fn install() -> Result<Value> {
    use std::os::unix::fs::PermissionsExt;
    let dir = extensions_dir().join(EXTENSION_UUID);
    std::fs::create_dir_all(&dir).map_err(|e| AdlError::internal(format!("{}: {e}", dir.display())))?;
    for (name, text) in FILES {
        let p = dir.join(name);
        std::fs::write(&p, text).map_err(|e| AdlError::internal(format!("{}: {e}", p.display())))?;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
    }
    let (enabled, mut notes) = match enable() {
        Ok(n) => (true, n),
        Err(e) => (false, vec![format!("Could not enable it ({e}); run `gnome-extensions enable {EXTENSION_UUID}`.")]),
    };
    let loaded = adl_linux::gnome::extension_present().is_ok();
    notes.push(if loaded {
        "The running GNOME Shell has an earlier copy loaded; log out and back in to load these files.".into()
    } else {
        "GNOME Shell loads newly installed extensions only when it starts: log out and back in (on X11, Alt+F2 then `r` also works), then restart `w3bs desktop daemon` if it was already running.".into()
    });
    let message = format!(
        "Installed the Agentic Desktop Layer GNOME Shell extension to {}{}.\n{}",
        dir.display(),
        if enabled { " and enabled it" } else { "" },
        notes.join("\n")
    );
    Ok(json!({"uuid": EXTENSION_UUID, "path": dir, "enabled": enabled, "loaded_now": loaded, "message": message}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strv_round_trip() {
        assert!(parse_strv("@as []").is_empty());
        assert_eq!(parse_strv("['a@b.org', 'agentic-desktop@w3bs.org']"), ["a@b.org", "agentic-desktop@w3bs.org"]);
        assert_eq!(format_strv(&["x@y".into(), "z@w".into()]), "['x@y', 'z@w']");
        assert_eq!(format_strv(&[]), "[]");
    }

    #[test]
    fn bundles_the_extension() {
        let meta: Value = serde_json::from_str(FILES[0].1).unwrap();
        assert_eq!(meta["uuid"], EXTENSION_UUID);
        assert!(meta["shell-version"].as_array().unwrap().iter().any(|v| v == "50"));
        assert!(FILES[1].1.contains("org.w3bs.Desktop.Shell") && FILES[1].1.contains("native_control"));
    }
}
