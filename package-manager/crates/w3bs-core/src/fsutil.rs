//! Durable filesystem primitives.

use crate::error::{IoContext, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

pub fn fsync_dir(dir: &Path) {
    if let Ok(f) = File::open(dir) {
        let _ = f.sync_all();
    }
}

/// Write via temp file + fsync + rename + directory fsync.
pub fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().expect("parent");
    std::fs::create_dir_all(dir).ctx(|| format!("create {}", dir.display()))?;
    let tmp = dir.join(format!(".{}.tmp-{}", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    {
        let mut f = File::create(&tmp).ctx(|| format!("create {}", tmp.display()))?;
        f.write_all(bytes).ctx(|| format!("write {}", tmp.display()))?;
        f.sync_all().ctx(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).ctx(|| format!("rename into {}", path.display()))?;
    fsync_dir(dir);
    Ok(())
}

/// Atomically point `link` at `target` (symlink swap via rename).
#[cfg(unix)]
pub fn swap_symlink(link: &Path, target: &Path) -> Result<()> {
    let dir = link.parent().expect("parent");
    let tmp = dir.join(format!(".{}.swap-{}", link.file_name().unwrap().to_string_lossy(), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).ctx(|| format!("symlink {}", tmp.display()))?;
    std::fs::rename(&tmp, link).ctx(|| format!("activate {}", link.display()))?;
    fsync_dir(dir);
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).ctx(|| format!("open {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ctx(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

pub fn sha256_bytes(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Sync every file and directory beneath `root`, then `root`'s parent.
pub fn sync_tree(root: &Path) -> Result<()> {
    fn walk(p: &Path) -> Result<()> {
        let md = std::fs::symlink_metadata(p).ctx(|| format!("stat {}", p.display()))?;
        if md.is_dir() {
            for e in std::fs::read_dir(p).ctx(|| format!("read {}", p.display()))? {
                walk(&e.ctx(|| p.display().to_string())?.path())?;
            }
            fsync_dir(p);
        } else if md.is_file() {
            File::open(p).and_then(|f| f.sync_all()).ctx(|| format!("fsync {}", p.display()))?;
        }
        Ok(())
    }
    walk(root)?;
    if let Some(parent) = root.parent() {
        fsync_dir(parent);
    }
    Ok(())
}
