//! Staging, activation and journaled recovery.
//!
//! Order of durable steps for one package:
//!   1. extract + verify into `packages/<key>/.staging-<txn>` (fsynced)
//!   2. write journal (old target, new target, links)        -> checkpoint `journal`
//!   3. rename staging -> `versions/<version>/<digest>`       -> checkpoint `staged`
//!   4. swap `current` symlink                                -> checkpoint `activated`
//!   5. swap launch links                                     -> checkpoint `linked`
//!   6. commit the database transaction                       -> checkpoint `committed`
//!   7. write receipt, delete journal
//!
//! Recovery: if the database already names the new directory as active, roll forward
//! (re-point `current` and links); otherwise roll back to the committed state. Either way
//! the result is the previous committed installation or the verified new one, never a mix.

use super::{Engine, checkpoint};
use crate::db::{self, LinkRow, PackageRow};
use crate::error::{Error, IoContext, Result};
use crate::fsutil;
use crate::layout::Layout;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub transaction_id: String,
    pub operation: String,
    pub package_id: String,
    pub package_key: String,
    /// `current` target before (relative to the package dir), if any.
    pub old_target: Option<String>,
    /// `current` target after.
    pub new_target: String,
    /// Staging directory to promote, relative to the package dir (None for rollback).
    pub staging: Option<String>,
    pub new_links: Vec<(String, String)>,
}

pub fn pending_journals(layout: &Layout) -> Result<Vec<PathBuf>> {
    let dir = layout.journal_dir();
    let mut v = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                v.push(e.path());
            }
        }
    }
    v.sort();
    Ok(v)
}

fn journal_path(layout: &Layout, j: &Journal) -> PathBuf {
    layout.journal_dir().join(format!("{}-{}.json", j.transaction_id, j.package_key))
}

pub fn write_journal(layout: &Layout, j: &Journal) -> Result<()> {
    fsutil::write_durable(&journal_path(layout, j), &serde_json::to_vec_pretty(j).unwrap())
}

pub fn clear_journal(layout: &Layout, j: &Journal) {
    let _ = std::fs::remove_file(journal_path(layout, j));
    fsutil::fsync_dir(&layout.journal_dir());
}

pub fn current_target(pkg_dir: &Path) -> Option<String> {
    std::fs::read_link(pkg_dir.join("current")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// Absolute target a launch link should point at.
pub fn link_target(layout: &Layout, key: &str, rel: &str) -> PathBuf {
    layout.package_dir(key).join("current").join(rel)
}

pub fn launch_links(layout: &Layout, key: &str, commands: &BTreeMap<String, String>) -> Vec<(String, String)> {
    commands
        .iter()
        .map(|(cmd, rel)| {
            (layout.bin_dir.join(cmd).to_string_lossy().into_owned(), link_target(layout, key, rel).to_string_lossy().into_owned())
        })
        .collect()
}

/// Whether a filesystem path is a symlink pointing exactly at `target`.
pub fn link_is(path: &Path, target: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) && std::fs::read_link(path).is_ok_and(|t| t.to_string_lossy() == target)
}

/// Apply links: create/replace links we own, delete links of commands the package dropped.
/// Refuses (before touching anything) to replace a path we do not own.
pub fn apply_links(conn: &rusqlite::Connection, package_id: &str, old: &[LinkRow], new: &[(String, String)]) -> Result<()> {
    for (path, _) in new {
        let p = Path::new(path);
        if std::fs::symlink_metadata(p).is_ok() {
            let owned_by_us = db::link_owner(conn, path)?.as_deref() == Some(package_id);
            if !owned_by_us && !new.iter().any(|(np, nt)| np == path && link_is(p, nt)) {
                return Err(Error::package("COMMAND_CONFLICT", format!("{path} already exists and is not managed by this package; refusing to overwrite it")));
            }
        }
    }
    for (path, target) in new {
        let p = Path::new(path);
        if !link_is(p, target) {
            fsutil::swap_symlink(p, Path::new(target))?;
        }
    }
    for l in old {
        if !new.iter().any(|(p, _)| *p == l.path) && link_is(Path::new(&l.path), &l.target) {
            let _ = std::fs::remove_file(&l.path);
        }
    }
    Ok(())
}

/// Reconcile every pending journal. Called under the mutation lock.
pub fn recover_all(engine: &mut Engine) -> Result<Vec<String>> {
    let mut done = vec![];
    for path in pending_journals(&engine.layout)? {
        let bytes = std::fs::read(&path).ctx(|| format!("read {}", path.display()))?;
        let j: Journal = serde_json::from_slice(&bytes)
            .map_err(|e| Error::preflight("CORRUPT_JOURNAL", format!("{} is unreadable ({e}); inspect it and run `w3bs doctor`", path.display())))?;
        recover_one(engine, &j)?;
        let _ = std::fs::remove_file(&path);
        done.push(format!("{} {} ({})", j.operation, j.package_id, j.transaction_id));
    }
    fsutil::fsync_dir(&engine.layout.journal_dir());
    Ok(done)
}

fn recover_one(engine: &mut Engine, j: &Journal) -> Result<()> {
    let layout = engine.layout.clone();
    let pkg_dir = layout.package_dir(&j.package_key);
    let conn = engine.conn();
    let row: Option<PackageRow> = db::package(conn, &j.package_id)?;
    let committed_dir = row.as_ref().map(|r| r.active_dir.clone());
    let new_dir_rel = format!("packages/{}/{}", j.package_key, j.new_target);
    if committed_dir.as_deref() == Some(new_dir_rel.as_str()) {
        // Roll forward: the database says the new version is active.
        fsutil::swap_symlink(&pkg_dir.join("current"), Path::new(&j.new_target))?;
        let old = db::links(conn, &j.package_id)?;
        apply_links(conn, &j.package_id, &old, &j.new_links)?;
    } else {
        // Roll back to the committed state.
        match (&j.old_target, &row) {
            (Some(old), Some(_)) => fsutil::swap_symlink(&pkg_dir.join("current"), Path::new(old))?,
            _ => {
                let _ = std::fs::remove_file(pkg_dir.join("current"));
            }
        }
        let committed_links = db::links(conn, &j.package_id)?;
        for (path, target) in &j.new_links {
            if !committed_links.iter().any(|l| l.path == *path) && link_is(Path::new(path), target) {
                let _ = std::fs::remove_file(path);
            }
        }
        let wanted: Vec<(String, String)> = committed_links.iter().map(|l| (l.path.clone(), l.target.clone())).collect();
        apply_links(conn, &j.package_id, &committed_links, &wanted)?;
        // Remove the new version directory unless the committed state references it.
        let referenced = row.as_ref().is_some_and(|r| r.active_dir == new_dir_rel || r.previous_dir.as_deref() == Some(new_dir_rel.as_str()));
        if !referenced && j.staging.is_some() {
            let _ = std::fs::remove_dir_all(layout.data_dir.join(&new_dir_rel));
        }
        if row.is_none() {
            let _ = std::fs::remove_dir_all(&pkg_dir);
        }
    }
    if let Some(s) = &j.staging {
        let _ = std::fs::remove_dir_all(pkg_dir.join(s));
    }
    Ok(())
}

/// Promote a verified staging directory and activate it. Returns the new dir (relative to
/// the data dir). The caller commits the database afterwards.
pub fn promote_and_activate(engine: &Engine, j: &Journal) -> Result<()> {
    let layout = &engine.layout;
    let pkg_dir = layout.package_dir(&j.package_key);
    write_journal(layout, j)?;
    checkpoint("journal");
    if let Some(staging) = &j.staging {
        let dest = pkg_dir.join(&j.new_target);
        std::fs::create_dir_all(dest.parent().unwrap()).ctx(|| format!("create {}", dest.display()))?;
        if dest.exists() {
            return Err(Error::package("VERSION_DIR_EXISTS", format!("{} already exists; run `w3bs doctor`", dest.display())));
        }
        std::fs::rename(pkg_dir.join(staging), &dest).ctx(|| format!("promote {}", dest.display()))?;
        fsutil::fsync_dir(dest.parent().unwrap());
    }
    checkpoint("staged");
    fsutil::swap_symlink(&pkg_dir.join("current"), Path::new(&j.new_target))?;
    checkpoint("activated");
    let old = db::links(engine.conn(), &j.package_id)?;
    apply_links(engine.conn(), &j.package_id, &old, &j.new_links)?;
    checkpoint("linked");
    Ok(())
}
