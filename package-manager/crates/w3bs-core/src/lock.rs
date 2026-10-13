//! One mutating owner per user store.

use crate::error::{Error, Kind, Result};
use fs4::fs_std::FileExt;
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

pub struct MutationLock {
    file: File,
}

impl MutationLock {
    /// Wait up to `timeout` for the lock, then fail with exit code 5.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| Error::preflight("LOCK_FILE", format!("cannot open {}: {e}", path.display())))?;
        let start = Instant::now();
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(true) => return Ok(Self { file }),
                Ok(false) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(Error::preflight("LOCK_FILE", format!("cannot lock {}: {e}", path.display()))),
            }
            if start.elapsed() >= timeout {
                return Err(Error::new(
                    Kind::Locked,
                    "LOCKED",
                    format!("another w3bs process holds the mutation lock ({}); try again later", path.display()),
                )
                .retryable());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn timeout_from_env() -> Duration {
    std::env::var("W3BS_LOCK_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).map(Duration::from_secs).unwrap_or(Duration::from_secs(30))
}
