use crate::transport::Endpoint;
use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(not(windows))]
fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

/// Per-user local application data on Windows (`%LOCALAPPDATA%\w3bs\desktop`).
#[cfg(windows)]
fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData\\Local")).join("w3bs\\desktop")
}

/// Sockets live in a per-user directory only the user can enter (Linux/macOS).
#[cfg_attr(windows, allow(dead_code))]
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("W3BS_DESKTOP_RUNTIME_DIR") {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        local_app_data().join("run")
    }
    #[cfg(not(windows))]
    {
        // macOS has no XDG_RUNTIME_DIR; its per-user temporary directory is already private.
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| user_temp_dir().join(format!("w3bs-desktop-{}", crate::peer::my_uid())));
        base.join("w3bs-desktop")
    }
}

/// Where recordings waiting for W3BS Training are kept across a restart of the broker: only a
/// memory-backed (tmpfs) runtime directory qualifies, because recordings are never written to a
/// disk unencrypted. Linux's `XDG_RUNTIME_DIR` is tmpfs; elsewhere this is None.
pub fn memory_backed_spill_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        const TMPFS_MAGIC: i64 = 0x0102_1994;
        let dir = runtime_dir();
        let probe = dir.parent().unwrap_or(&dir);
        let c = std::ffi::CString::new(probe.as_os_str().as_encoded_bytes()).ok()?;
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: statfs fills `st` for a NUL-terminated path.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 || st.f_type as i64 != TMPFS_MAGIC {
            return None;
        }
        Some(dir.join("training-pending"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The per-user temporary directory. On macOS this asks the system (`confstr`) rather than
/// reading `$TMPDIR`, which launchd does not set for LaunchAgents: the broker started at login
/// and the CLI started from a terminal must agree on where the sockets are.
#[cfg(target_os = "macos")]
fn user_temp_dir() -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; 1024];
    // SAFETY: confstr writes at most buf.len() bytes, NUL-terminated, and returns the length it
    // needs including the NUL (0 on error).
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    if n > 1 && n <= buf.len() {
        return PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n - 1]));
    }
    std::env::temp_dir()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn user_temp_dir() -> PathBuf {
    std::env::temp_dir()
}

#[cfg(not(windows))]
fn endpoint(name: &str) -> Endpoint {
    Endpoint { name: runtime_dir().join(format!("{name}.sock")) }
}

/// Windows: a per-user pipe name. `W3BS_DESKTOP_RUNTIME_DIR` (tests) folds into the name so
/// parallel brokers do not collide.
#[cfg(windows)]
fn endpoint(name: &str) -> Endpoint {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let scope = std::env::var("W3BS_DESKTOP_RUNTIME_DIR").map(|d| format!("-{:x}", d.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)))).unwrap_or_default();
    Endpoint { name: PathBuf::from(format!(r"\\.\pipe\w3bs-desktop-{user}{scope}-{name}")) }
}

pub fn agent_socket() -> Endpoint {
    endpoint("agent")
}

pub fn control_socket() -> Endpoint {
    endpoint("control")
}

pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("W3BS_DESKTOP_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        local_app_data().join("config")
    }
    #[cfg(not(windows))]
    {
        xdg("XDG_CONFIG_HOME", ".config").join("w3bs/desktop")
    }
}

pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("W3BS_DESKTOP_STATE_DIR") {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        local_app_data().join("state")
    }
    #[cfg(not(windows))]
    {
        xdg("XDG_STATE_HOME", ".local/state").join("w3bs/desktop")
    }
}

pub fn token_file(agent: &str) -> PathBuf {
    config_dir().join("tokens").join(format!("{agent}.token"))
}
