//! Transactional local state. The database is the source of truth for committed
//! installations; receipts are views of it.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS repositories(
  alias TEXT PRIMARY KEY,
  repo_key TEXT NOT NULL UNIQUE,
  base_url TEXT NOT NULL,
  root_fingerprint TEXT NOT NULL,
  added_at TEXT NOT NULL,
  refreshed_at TEXT
);
-- Survives `repo remove`, so a re-added repository is never treated as a new identity.
CREATE TABLE IF NOT EXISTS trust_history(
  repo_key TEXT PRIMARY KEY,
  base_url TEXT NOT NULL,
  initial_fingerprint TEXT NOT NULL,
  first_trusted_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS packages(
  package_id TEXT PRIMARY KEY,
  package_key TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  repo_alias TEXT NOT NULL,
  repo_key TEXT NOT NULL,
  root_fingerprint TEXT NOT NULL,
  channel TEXT NOT NULL,
  update_policy TEXT NOT NULL,
  pinned_version TEXT,
  active_version TEXT NOT NULL,
  active_digest TEXT NOT NULL,
  active_dir TEXT NOT NULL,
  previous_version TEXT,
  previous_digest TEXT,
  previous_dir TEXT,
  platform TEXT NOT NULL,
  commands TEXT NOT NULL,
  installed_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin TEXT,
  transaction_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS owned_files(
  package_id TEXT NOT NULL,
  dir TEXT NOT NULL,
  rel TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  PRIMARY KEY(package_id, dir, rel)
);
CREATE TABLE IF NOT EXISTS launch_links(
  package_id TEXT NOT NULL,
  command TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,
  target TEXT NOT NULL,
  PRIMARY KEY(package_id, command)
);
CREATE TABLE IF NOT EXISTS observed_versions(
  repo_key TEXT NOT NULL,
  package_id TEXT NOT NULL,
  version TEXT NOT NULL,
  platform TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  PRIMARY KEY(repo_key, package_id, version, platform)
);
CREATE TABLE IF NOT EXISTS revocations(
  repo_key TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  reason TEXT NOT NULL,
  PRIMARY KEY(repo_key, sha256)
);
-- One row per installed version directory still on disk (active, previous, retained).
CREATE TABLE IF NOT EXISTS versions(
  package_id TEXT NOT NULL,
  dir TEXT NOT NULL,
  version TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  commands TEXT NOT NULL,
  PRIMARY KEY(package_id, dir)
);
-- Tools installed by running their publisher's curl installer. W3BS records them but does
-- not own their files, so it cannot verify, roll back or delete those files.
CREATE TABLE IF NOT EXISTS scripted(
  package_id TEXT PRIMARY KEY,
  description TEXT NOT NULL DEFAULT '',
  repo_alias TEXT NOT NULL,
  repo_key TEXT NOT NULL,
  install_url TEXT NOT NULL,
  shell TEXT NOT NULL,
  script_sha256 TEXT NOT NULL,
  commands TEXT NOT NULL,
  installed_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  transaction_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS history(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  package_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  from_version TEXT,
  to_version TEXT,
  at TEXT NOT NULL,
  transaction_id TEXT NOT NULL
);
"#;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA)?;
    conn.execute("INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', '1')", [])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(conn)
}

/// Read-only open that does not create anything; `None` when there is no state yet.
pub fn open_existing(path: &Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    open(path).map(Some)
}

#[derive(Debug, Clone)]
pub struct RepoRow {
    pub alias: String,
    pub repo_key: String,
    pub base_url: String,
    pub root_fingerprint: String,
    pub added_at: String,
    pub refreshed_at: Option<String>,
}

pub fn repos(conn: &Connection) -> Result<Vec<RepoRow>> {
    let mut st = conn.prepare("SELECT alias, repo_key, base_url, root_fingerprint, added_at, refreshed_at FROM repositories ORDER BY alias")?;
    let rows = st
        .query_map([], |r| {
            Ok(RepoRow {
                alias: r.get(0)?,
                repo_key: r.get(1)?,
                base_url: r.get(2)?,
                root_fingerprint: r.get(3)?,
                added_at: r.get(4)?,
                refreshed_at: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn repo(conn: &Connection, alias: &str) -> Result<Option<RepoRow>> {
    Ok(repos(conn)?.into_iter().find(|r| r.alias == alias))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageRow {
    pub package_id: String,
    pub package_key: String,
    pub description: String,
    pub repo_alias: String,
    pub repo_key: String,
    pub root_fingerprint: String,
    pub channel: String,
    pub update_policy: String,
    pub pinned_version: Option<String>,
    pub active_version: String,
    pub active_digest: String,
    pub active_dir: String,
    pub previous_version: Option<String>,
    pub previous_digest: Option<String>,
    pub previous_dir: Option<String>,
    pub platform: String,
    pub commands: Vec<String>,
    pub installed_at: String,
    pub updated_at: String,
    pub origin: Option<String>,
    pub transaction_id: String,
}

const PKG_COLS: &str = "package_id, package_key, description, repo_alias, repo_key, root_fingerprint, channel, update_policy, pinned_version, active_version, active_digest, active_dir, previous_version, previous_digest, previous_dir, platform, commands, installed_at, updated_at, origin, transaction_id";

fn pkg_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PackageRow> {
    let commands: String = r.get(16)?;
    Ok(PackageRow {
        package_id: r.get(0)?,
        package_key: r.get(1)?,
        description: r.get(2)?,
        repo_alias: r.get(3)?,
        repo_key: r.get(4)?,
        root_fingerprint: r.get(5)?,
        channel: r.get(6)?,
        update_policy: r.get(7)?,
        pinned_version: r.get(8)?,
        active_version: r.get(9)?,
        active_digest: r.get(10)?,
        active_dir: r.get(11)?,
        previous_version: r.get(12)?,
        previous_digest: r.get(13)?,
        previous_dir: r.get(14)?,
        platform: r.get(15)?,
        commands: serde_json::from_str(&commands).unwrap_or_default(),
        installed_at: r.get(17)?,
        updated_at: r.get(18)?,
        origin: r.get(19)?,
        transaction_id: r.get(20)?,
    })
}

pub fn packages(conn: &Connection) -> Result<Vec<PackageRow>> {
    let mut st = conn.prepare(&format!("SELECT {PKG_COLS} FROM packages ORDER BY package_id"))?;
    let rows = st.query_map([], pkg_from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn package(conn: &Connection, id: &str) -> Result<Option<PackageRow>> {
    Ok(conn.query_row(&format!("SELECT {PKG_COLS} FROM packages WHERE package_id = ?1"), [id], pkg_from_row).optional()?)
}

pub fn upsert_package(conn: &Connection, p: &PackageRow) -> Result<()> {
    conn.execute(
        &format!("INSERT OR REPLACE INTO packages({PKG_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)"),
        params![
            p.package_id,
            p.package_key,
            p.description,
            p.repo_alias,
            p.repo_key,
            p.root_fingerprint,
            p.channel,
            p.update_policy,
            p.pinned_version,
            p.active_version,
            p.active_digest,
            p.active_dir,
            p.previous_version,
            p.previous_digest,
            p.previous_dir,
            p.platform,
            serde_json::to_string(&p.commands).unwrap(),
            p.installed_at,
            p.updated_at,
            p.origin,
            p.transaction_id
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LinkRow {
    pub command: String,
    pub path: String,
    pub target: String,
}

pub fn links(conn: &Connection, package_id: &str) -> Result<Vec<LinkRow>> {
    let mut st = conn.prepare("SELECT command, path, target FROM launch_links WHERE package_id = ?1 ORDER BY command")?;
    let rows = st
        .query_map([package_id], |r| Ok(LinkRow { command: r.get(0)?, path: r.get(1)?, target: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn link_owner(conn: &Connection, path: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT package_id FROM launch_links WHERE path = ?1", [path], |r| r.get(0)).optional()?)
}

pub fn owned_files(conn: &Connection, package_id: &str, dir: &str) -> Result<Vec<(String, String)>> {
    let mut st = conn.prepare("SELECT rel, sha256 FROM owned_files WHERE package_id = ?1 AND dir = ?2 ORDER BY rel")?;
    let rows = st.query_map(params![package_id, dir], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn history(conn: &Connection, package_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut st = conn.prepare("SELECT operation, from_version, to_version, at, transaction_id FROM history WHERE package_id = ?1 ORDER BY id")?;
    let rows = st
        .query_map([package_id], |r| {
            Ok(serde_json::json!({
                "operation": r.get::<_, String>(0)?,
                "from": r.get::<_, Option<String>>(1)?,
                "to": r.get::<_, Option<String>>(2)?,
                "at": r.get::<_, String>(3)?,
                "transaction_id": r.get::<_, String>(4)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn record_history(conn: &Connection, id: &str, op: &str, from: Option<&str>, to: Option<&str>, txn: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO history(package_id, operation, from_version, to_version, at, transaction_id) VALUES (?1,?2,?3,?4,?5,?6)",
        params![id, op, from, to, crate::now(), txn],
    )?;
    Ok(())
}

pub fn observed_digest(conn: &Connection, repo_key: &str, id: &str, version: &str, platform: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT sha256 FROM observed_versions WHERE repo_key=?1 AND package_id=?2 AND version=?3 AND platform=?4",
            params![repo_key, id, version, platform],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn record_observed(conn: &Connection, repo_key: &str, id: &str, version: &str, platform: &str, sha: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO observed_versions(repo_key, package_id, version, platform, sha256) VALUES (?1,?2,?3,?4,?5)",
        params![repo_key, id, version, platform, sha],
    )?;
    Ok(())
}

pub fn record_revocation(conn: &Connection, repo_key: &str, sha: &str, reason: &str) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO revocations(repo_key, sha256, reason) VALUES (?1,?2,?3)", params![repo_key, sha, reason])?;
    Ok(())
}

pub fn revocation(conn: &Connection, repo_key: &str, sha: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT reason FROM revocations WHERE repo_key=?1 AND sha256=?2", params![repo_key, sha], |r| r.get(0))
        .optional()?)
}

#[derive(Debug, Clone)]
pub struct VersionRow {
    pub dir: String,
    pub version: String,
    pub sha256: String,
    pub commands: std::collections::BTreeMap<String, String>,
}

pub fn version_row(conn: &Connection, package_id: &str, dir: &str) -> Result<Option<VersionRow>> {
    Ok(conn
        .query_row(
            "SELECT dir, version, sha256, commands FROM versions WHERE package_id=?1 AND dir=?2",
            params![package_id, dir],
            |r| {
                let c: String = r.get(3)?;
                Ok(VersionRow { dir: r.get(0)?, version: r.get(1)?, sha256: r.get(2)?, commands: serde_json::from_str(&c).unwrap_or_default() })
            },
        )
        .optional()?)
}

pub fn all_version_dirs(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut st = conn.prepare("SELECT package_id, dir, sha256 FROM versions")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn delete_version(conn: &Connection, package_id: &str, dir: &str) -> Result<()> {
    conn.execute("DELETE FROM versions WHERE package_id=?1 AND dir=?2", params![package_id, dir])?;
    conn.execute("DELETE FROM owned_files WHERE package_id=?1 AND dir=?2", params![package_id, dir])?;
    Ok(())
}

pub fn delete_package(conn: &Connection, package_id: &str) -> Result<()> {
    for t in ["packages", "owned_files", "launch_links", "versions"] {
        conn.execute(&format!("DELETE FROM {t} WHERE package_id=?1"), [package_id])?;
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScriptedRow {
    pub package_id: String,
    pub description: String,
    pub repo_alias: String,
    pub repo_key: String,
    pub install_url: String,
    pub shell: String,
    pub script_sha256: String,
    pub commands: Vec<String>,
    pub installed_at: String,
    pub updated_at: String,
    pub transaction_id: String,
}

const SCRIPTED_COLS: &str = "package_id, description, repo_alias, repo_key, install_url, shell, script_sha256, commands, installed_at, updated_at, transaction_id";

fn scripted_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScriptedRow> {
    let c: String = r.get(7)?;
    Ok(ScriptedRow {
        package_id: r.get(0)?,
        description: r.get(1)?,
        repo_alias: r.get(2)?,
        repo_key: r.get(3)?,
        install_url: r.get(4)?,
        shell: r.get(5)?,
        script_sha256: r.get(6)?,
        commands: serde_json::from_str(&c).unwrap_or_default(),
        installed_at: r.get(8)?,
        updated_at: r.get(9)?,
        transaction_id: r.get(10)?,
    })
}

pub fn scripted_all(conn: &Connection) -> Result<Vec<ScriptedRow>> {
    let mut st = conn.prepare(&format!("SELECT {SCRIPTED_COLS} FROM scripted ORDER BY package_id"))?;
    let rows = st.query_map([], scripted_from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn scripted(conn: &Connection, id: &str) -> Result<Option<ScriptedRow>> {
    Ok(conn.query_row(&format!("SELECT {SCRIPTED_COLS} FROM scripted WHERE package_id=?1"), [id], scripted_from_row).optional()?)
}

pub fn upsert_scripted(conn: &Connection, s: &ScriptedRow) -> Result<()> {
    conn.execute(
        &format!("INSERT OR REPLACE INTO scripted({SCRIPTED_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"),
        params![
            s.package_id,
            s.description,
            s.repo_alias,
            s.repo_key,
            s.install_url,
            s.shell,
            s.script_sha256,
            serde_json::to_string(&s.commands).unwrap(),
            s.installed_at,
            s.updated_at,
            s.transaction_id
        ],
    )?;
    Ok(())
}

pub fn delete_scripted(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM scripted WHERE package_id=?1", [id])?;
    Ok(())
}
