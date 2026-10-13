use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Random identifier with a readable prefix, e.g. `task_3f9a…`.
pub fn random_id(prefix: &str, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS randomness unavailable");
    format!("{prefix}_{}", hex::encode(buf))
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Constant-time comparison for secrets of equal length.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Open options for a file only the user may read (0600 on Unix; Windows relies on the private
/// per-user directory it lives in).
pub fn private_file() -> std::fs::OpenOptions {
    #[allow(unused_mut)]
    let mut o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}
