use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

/// Sockets live in the per-user runtime directory, which only the user can enter.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("W3BS_DESKTOP_RUNTIME_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("w3bs-desktop-{}", crate::peer::my_uid())));
    base.join("w3bs-desktop")
}

pub fn agent_socket() -> PathBuf {
    runtime_dir().join("agent.sock")
}

pub fn control_socket() -> PathBuf {
    runtime_dir().join("control.sock")
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("W3BS_DESKTOP_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| xdg("XDG_CONFIG_HOME", ".config").join("w3bs/desktop"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("W3BS_DESKTOP_STATE_DIR").map(PathBuf::from).unwrap_or_else(|| xdg("XDG_STATE_HOME", ".local/state").join("w3bs/desktop"))
}

pub fn token_file(agent: &str) -> PathBuf {
    config_dir().join("tokens").join(format!("{agent}.token"))
}
