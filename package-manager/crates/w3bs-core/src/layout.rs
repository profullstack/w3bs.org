//! Where W3BS keeps things. Defaults follow XDG on Linux and macOS alike:
//!
//! ```text
//! ~/.config/w3bs/config.toml
//! ~/.local/share/w3bs/state.sqlite3
//! ~/.local/share/w3bs/trust/<repository-key>/
//! ~/.local/share/w3bs/receipts/<package-key>.json
//! ~/.local/share/w3bs/packages/<package-key>/versions/<version>/<digest>/
//! ~/.local/share/w3bs/packages/<package-key>/current
//! ~/.local/state/w3bs/journal/
//! ~/.cache/w3bs/blobs/sha256/<digest>
//! ~/.local/bin/<command>
//! ```
//!
//! `W3BS_HOME=<dir>` relocates everything under one directory (config, data, state, cache,
//! bin). Development transport (file:// and loopback http) is only honoured together with
//! an explicit `W3BS_HOME`, so it can never touch the production user store.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Layout {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub bin_dir: PathBuf,
    /// Allow file:// and loopback http repositories (tests and local development only).
    pub dev_transport: bool,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute())
}

impl Layout {
    pub fn from_env() -> Result<Self> {
        let dev = std::env::var("W3BS_DEV_INSECURE_TRANSPORT").is_ok_and(|v| v == "1");
        if let Some(home) = env_path("W3BS_HOME") {
            return Ok(Self {
                config_dir: home.join("config"),
                data_dir: home.join("data"),
                state_dir: home.join("state"),
                cache_dir: home.join("cache"),
                bin_dir: env_path("W3BS_BIN_DIR").unwrap_or_else(|| home.join("bin")),
                dev_transport: dev,
            });
        }
        if dev {
            return Err(Error::preflight(
                "DEV_TRANSPORT_NEEDS_HOME",
                "W3BS_DEV_INSECURE_TRANSPORT=1 is only allowed with an explicit W3BS_HOME, never on the production user store",
            ));
        }
        let home = env_path("HOME").ok_or_else(|| Error::preflight("NO_HOME", "HOME is not set to an absolute path"))?;
        let xdg = |var: &str, default: &str| env_path(var).unwrap_or_else(|| home.join(default)).join("w3bs");
        Ok(Self {
            config_dir: xdg("XDG_CONFIG_HOME", ".config"),
            data_dir: xdg("XDG_DATA_HOME", ".local/share"),
            state_dir: xdg("XDG_STATE_HOME", ".local/state"),
            cache_dir: xdg("XDG_CACHE_HOME", ".cache"),
            bin_dir: env_path("W3BS_BIN_DIR").unwrap_or_else(|| home.join(".local/bin")),
            dev_transport: false,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("state.sqlite3")
    }
    pub fn trust_dir(&self, repo_key: &str) -> PathBuf {
        self.data_dir.join("trust").join(repo_key)
    }
    pub fn receipts_dir(&self) -> PathBuf {
        self.data_dir.join("receipts")
    }
    pub fn packages_dir(&self) -> PathBuf {
        self.data_dir.join("packages")
    }
    pub fn package_dir(&self, key: &str) -> PathBuf {
        self.packages_dir().join(key)
    }
    pub fn journal_dir(&self) -> PathBuf {
        self.state_dir.join("journal")
    }
    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join("lock")
    }
    pub fn blob_path(&self, sha256: &str) -> PathBuf {
        self.cache_dir.join("blobs/sha256").join(sha256)
    }

    /// Create private directories. Called only by mutating operations.
    pub fn ensure(&self) -> Result<()> {
        for d in [&self.config_dir, &self.data_dir, &self.state_dir, &self.cache_dir, &self.journal_dir(), &self.packages_dir(), &self.receipts_dir()] {
            create_private_dir(d)?;
        }
        std::fs::create_dir_all(&self.bin_dir)
            .map_err(|e| Error::preflight("BIN_DIR", format!("cannot create {}: {e}", self.bin_dir.display())))?;
        check_local_filesystem(&self.data_dir)?;
        Ok(())
    }
}

pub fn create_private_dir(d: &Path) -> Result<()> {
    std::fs::create_dir_all(d).map_err(|e| Error::preflight("STATE_DIR", format!("cannot create {}: {e}", d.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Reject network filesystems for the store: activation relies on same-filesystem atomic
/// rename and fsync semantics that were only qualified on local filesystems.
pub fn check_local_filesystem(p: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(p.as_os_str().as_bytes()).map_err(|_| Error::preflight("STORE_PATH", "store path contains NUL"))?;
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut s) } == 0 {
            // NFS, SMB/CIFS, SMB2, AFS, Coda, 9p.
            const NETWORK: &[i64] = &[0x6969, 0xFF53_4D42, 0xFE53_4D42, 0x5346_414F, 0x7375_7245, 0x0102_1997];
            if NETWORK.contains(&(s.f_type as i64)) {
                return Err(Error::preflight(
                    "UNSUPPORTED_FILESYSTEM",
                    format!("{} is on a network filesystem; W3BS needs a local filesystem for crash-safe activation", p.display()),
                ));
            }
        }
    }
    let _ = p;
    Ok(())
}
