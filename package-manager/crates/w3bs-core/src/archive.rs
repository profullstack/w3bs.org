//! Untrusted archive extraction into a staging directory.
//!
//! Rejects absolute and traversal paths, links that escape the staging root, hard links,
//! device/FIFO entries and duplicate paths; strips setuid/setgid/sticky bits; enforces an
//! expanded-size limit, a file-count limit and a path-length limit. Nothing is ever written
//! outside `dest`, and nothing is written through a symlink.

use crate::error::{Error, IoContext, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use w3bs_schema::identity::validate_relative_path;

pub const MAX_FILES: usize = 20_000;

fn bad(msg: impl Into<String>) -> Error {
    Error::package("UNSAFE_ARCHIVE", msg)
}

/// One extracted entry: relative path and `sha256` of its content (`symlink:<target>` for links).
pub type Entry = (String, String);

fn normalize_entry_path(raw: &str) -> Option<String> {
    let s = raw.strip_prefix("./").unwrap_or(raw);
    let s = s.strip_suffix('/').unwrap_or(s);
    if s.is_empty() || s == "." {
        return None; // the archive root itself
    }
    Some(s.to_owned())
}

/// Resolve a relative link target against the link's directory, purely lexically; `None` if it
/// escapes the root or is absolute.
fn resolve_link(link_rel: &str, target: &str) -> Option<PathBuf> {
    if target.starts_with('/') || target.is_empty() || target.contains('\0') {
        return None;
    }
    let mut stack: Vec<String> = link_rel.split('/').map(str::to_owned).collect();
    stack.pop();
    for c in Path::new(target).components() {
        match c {
            Component::Normal(n) => stack.push(n.to_str()?.to_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop()?;
            }
            _ => return None,
        }
    }
    Some(stack.iter().collect())
}

/// Refuse to write through an existing symlink anywhere along `rel`'s parent chain.
fn ensure_no_symlink_parents(dest: &Path, rel: &str) -> Result<()> {
    let mut p = dest.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        p.push(part);
        if let Ok(md) = std::fs::symlink_metadata(&p)
            && md.file_type().is_symlink()
        {
            return Err(bad(format!("entry `{rel}` would be written through a symlink")));
        }
    }
    Ok(())
}

pub fn extract_tar_gz(bytes: &[u8], dest: &Path, unpacked_limit: u64) -> Result<Vec<Entry>> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut ar = tar::Archive::new(gz);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut total: u64 = 0;
    let entries = ar.entries().map_err(|e| bad(format!("not a tar.gz archive: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| bad(format!("corrupt archive: {e}")))?;
        if seen.len() >= MAX_FILES {
            return Err(bad(format!("archive has more than {MAX_FILES} entries")));
        }
        let raw = String::from_utf8(entry.path_bytes().into_owned()).map_err(|_| bad("archive path is not UTF-8"))?;
        let Some(rel) = normalize_entry_path(&raw) else { continue };
        if !validate_relative_path(&rel) {
            return Err(bad(format!("unsafe archive path `{raw}`")));
        }
        if !seen.insert(rel.clone()) {
            return Err(bad(format!("duplicate archive path `{rel}`")));
        }
        let target = dest.join(&rel);
        ensure_no_symlink_parents(dest, &rel)?;
        let kind = entry.header().entry_type();
        let mode = entry.header().mode().unwrap_or(0o644);
        match kind {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&target).ctx(|| format!("create {}", target.display()))?;
                set_mode(&target, 0o755)?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let size = entry.header().size().unwrap_or(u64::MAX);
                total = total.saturating_add(size);
                if total > unpacked_limit {
                    return Err(bad(format!("archive expands beyond its declared limit of {unpacked_limit} bytes")));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).ctx(|| format!("create {}", parent.display()))?;
                }
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).ctx(|| format!("create {}", target.display()))?;
                let mut h = Sha256::new();
                let mut buf = [0u8; 64 * 1024];
                let mut written: u64 = 0;
                loop {
                    let n = entry.read(&mut buf).map_err(|e| bad(format!("corrupt archive entry `{rel}`: {e}")))?;
                    if n == 0 {
                        break;
                    }
                    written += n as u64;
                    if written > size {
                        return Err(bad(format!("entry `{rel}` is longer than its header says")));
                    }
                    h.update(&buf[..n]);
                    f.write_all(&buf[..n]).ctx(|| format!("write {}", target.display()))?;
                }
                drop(f);
                // Executable bits survive; setuid/setgid/sticky and group/other write never do.
                set_mode(&target, if mode & 0o111 != 0 { 0o755 } else { 0o644 })?;
                out.push((rel, hex::encode(h.finalize())));
            }
            tar::EntryType::Symlink => {
                let link = entry
                    .link_name_bytes()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .ok_or_else(|| bad(format!("symlink `{rel}` has no target")))?;
                if resolve_link(&rel, &link).is_none() {
                    return Err(bad(format!("symlink `{rel}` -> `{link}` escapes the package")));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).ctx(|| format!("create {}", parent.display()))?;
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(&link, &target).ctx(|| format!("symlink {}", target.display()))?;
                out.push((rel, format!("symlink:{link}")));
            }
            tar::EntryType::Link => return Err(bad(format!("hard link `{rel}` is not allowed"))),
            tar::EntryType::Char | tar::EntryType::Block | tar::EntryType::Fifo => {
                return Err(bad(format!("special file `{rel}` is not allowed")));
            }
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader | tar::EntryType::GNULongName | tar::EntryType::GNULongLink => {}
            other => return Err(bad(format!("unsupported archive entry type {other:?} for `{rel}`"))),
        }
    }
    Ok(out)
}

/// A raw single-binary artifact.
pub fn place_binary(bytes: &[u8], dest: &Path, rel: &str) -> Result<Vec<Entry>> {
    if !validate_relative_path(rel) {
        return Err(bad(format!("unsafe command path `{rel}`")));
    }
    let target = dest.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ctx(|| format!("create {}", parent.display()))?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).ctx(|| format!("create {}", target.display()))?;
    f.write_all(bytes).ctx(|| format!("write {}", target.display()))?;
    drop(f);
    set_mode(&target, 0o755)?;
    Ok(vec![(rel.to_owned(), crate::fsutil::sha256_bytes(bytes))])
}

fn set_mode(p: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).ctx(|| format!("chmod {}", p.display()))?;
    }
    let _ = (p, mode);
    Ok(())
}

/// Verify that `rel` names a regular file inside `root`, following only in-package symlinks.
pub fn resolve_command(root: &Path, entries: &[Entry], rel: &str) -> Result<()> {
    let mut cur = rel.to_owned();
    for _ in 0..16 {
        match entries.iter().find(|(p, _)| *p == cur) {
            Some((_, h)) if h.starts_with("symlink:") => {
                cur = resolve_link(&cur, &h["symlink:".len()..])
                    .and_then(|p| p.to_str().map(str::to_owned))
                    .ok_or_else(|| bad(format!("command `{rel}` resolves outside the package")))?;
            }
            Some(_) => {
                let md = std::fs::symlink_metadata(root.join(&cur)).ctx(|| format!("stat {cur}"))?;
                if !md.is_file() {
                    return Err(bad(format!("command `{rel}` is not a regular file")));
                }
                return Ok(());
            }
            None => return Err(Error::package("MISSING_COMMAND", format!("artifact does not contain declared command path `{rel}`"))),
        }
    }
    Err(bad(format!("command `{rel}` has too many symlink hops")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(build: impl FnOnce(&mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>)) -> Vec<u8> {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(enc);
        build(&mut b);
        b.into_inner().unwrap().finish().unwrap()
    }

    fn file(b: &mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>, path: &str, data: &[u8], mode: u32) {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(mode);
        h.set_entry_type(tar::EntryType::Regular);
        // set_path refuses `..`; write the raw name to simulate a hostile archive.
        let name = &mut h.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_cksum();
        b.append(&h, data).unwrap();
    }

    fn link(b: &mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>, path: &str, target: &str, kind: tar::EntryType) {
        let mut h = tar::Header::new_gnu();
        h.set_size(0);
        h.set_entry_type(kind);
        h.set_mode(0o777);
        let name = &mut h.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        let ln = &mut h.as_old_mut().linkname;
        ln[..target.len()].copy_from_slice(target.as_bytes());
        h.set_cksum();
        b.append(&h, &[][..]).unwrap();
    }

    #[test]
    fn extracts_and_strips_setuid() {
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| file(b, "./bin/atlas", b"#!/bin/sh\necho hi\n", 0o4755));
        let e = extract_tar_gz(&a, d.path(), 1000).unwrap();
        assert_eq!(e[0].0, "bin/atlas");
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(d.path().join("bin/atlas")).unwrap().permissions().mode() & 0o7777;
        assert_eq!(m, 0o755);
        resolve_command(d.path(), &e, "bin/atlas").unwrap();
    }

    #[test]
    fn rejects_traversal() {
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| file(b, "../evil", b"x", 0o644));
        assert!(extract_tar_gz(&a, &d.path().join("s"), 1000).is_err());
        assert!(!d.path().join("evil").exists());
        let a = tar_gz(|b| file(b, "/abs", b"x", 0o644));
        assert!(extract_tar_gz(&a, d.path(), 1000).is_err());
    }

    #[test]
    fn rejects_escaping_symlink_and_write_through() {
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| link(b, "bin/x", "../../etc/passwd", tar::EntryType::Symlink));
        assert!(extract_tar_gz(&a, d.path(), 1000).is_err());
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| {
            link(b, "lib", "bin", tar::EntryType::Symlink);
            file(b, "lib/x", b"x", 0o644);
        });
        assert!(extract_tar_gz(&a, d.path(), 1000).is_err());
    }

    #[test]
    fn rejects_hardlinks_devices_and_bombs() {
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| link(b, "x", "y", tar::EntryType::Link));
        assert!(extract_tar_gz(&a, d.path(), 1000).is_err());
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| link(b, "dev", "", tar::EntryType::Char));
        assert!(extract_tar_gz(&a, d.path(), 1000).is_err());
        let d = tempfile::tempdir().unwrap();
        let big = vec![0u8; 5000];
        let a = tar_gz(|b| file(b, "big", &big, 0o644));
        assert!(extract_tar_gz(&a, d.path(), 1000).is_err());
    }

    #[test]
    fn in_package_symlink_command_ok() {
        let d = tempfile::tempdir().unwrap();
        let a = tar_gz(|b| {
            file(b, "libexec/atlas-real", b"x", 0o755);
            link(b, "bin/atlas", "../libexec/atlas-real", tar::EntryType::Symlink);
        });
        let e = extract_tar_gz(&a, d.path(), 1000).unwrap();
        resolve_command(d.path(), &e, "bin/atlas").unwrap();
        assert!(resolve_command(d.path(), &e, "bin/missing").is_err());
    }
}
