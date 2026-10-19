//! Inventory, diagnostics, recovery, garbage collection and the manager's own lifecycle.

use super::activate;
use super::lifecycle::{InstallRequest, Installed, UpdateRequest};
use super::Engine;
use crate::db;
use crate::error::{Error, Result};
use crate::fsutil;
use crate::layout::Layout;
use crate::probe;
use serde_json::json;
use std::collections::BTreeSet;
use w3bs_schema::result::{OperationResult, Overall};
use w3bs_schema::{MANAGER_PACKAGE_ID, PackageRef};

/// Whether the running binary is the W3BS-managed manager installation.
pub fn manager_is_self_managed(layout: &Layout) -> bool {
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else { return false };
    layout.package_dir("w3bs-manager").canonicalize().is_ok_and(|d| exe.starts_with(d))
}

/// Whether this process runs in a container (Docker or Podman).
pub fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists() || std::path::Path::new("/run/.containerenv").exists()
}

/// How to get a newer w3bs when this binary is not the managed one. In a container the
/// binary is baked into the image (W3BS OS builds it for musl, which has no release).
pub fn external_upgrade_hint() -> String {
    if in_container() {
        return "pull the newer image (for W3BS OS: `docker pull w3bs.org/os:server`) and recreate the container".into();
    }
    external_owner_hint().unwrap_or_else(|| "reinstall with `curl -fsSL https://w3bs.org/install.sh | sh`".into())
}

/// Recognize an external owner of the running binary, for actionable instructions.
pub fn external_owner_hint() -> Option<String> {
    let exe = std::env::current_exe().ok()?.to_string_lossy().into_owned();
    Some(if exe.contains("/.cargo/bin/") {
        "installed with cargo: upgrade with `cargo install w3bs-cli --force`".into()
    } else if exe.contains("/Cellar/") || exe.contains("/homebrew/") || exe.contains("/linuxbrew/") {
        "installed with Homebrew: upgrade with `brew upgrade w3bs`".into()
    } else if exe.contains("/node_modules/") {
        "installed with npm: upgrade with `npm i -g @profullstack/w3bs`".into()
    } else {
        format!("this w3bs ({exe}) was not installed by W3BS; reinstall with `curl -fsSL https://w3bs.org/install.sh | sh`")
    })
}

impl Engine {
    pub fn list(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("list");
        let (native, scripted) = match self.read_state()? {
            Some(c) => (db::packages(c)?, db::scripted_all(c)?),
            None => (vec![], vec![]),
        };
        let mut items: Vec<serde_json::Value> = native
            .iter()
            .map(|p| {
                json!({
                    "id": p.package_id, "kind": "native", "version": p.active_version, "channel": p.channel,
                    "pinned": p.pinned_version, "repository": p.repo_alias, "commands": p.commands,
                    "owner": "w3bs", "updated_at": p.updated_at,
                })
            })
            .collect();
        // A curl tool's version is whatever the tool itself reports; W3BS does not own its files.
        let versions = probe::installed_versions(&scripted.iter().map(|s| s.commands.clone()).collect::<Vec<_>>(), &self.layout.bin_dir);
        items.extend(scripted.iter().zip(versions).map(|(s, version)| {
            json!({
                "id": s.package_id, "kind": "curl", "version": version, "channel": null, "pinned": null,
                "repository": s.repo_alias, "commands": s.commands, "owner": "publisher-installer",
                "installer": format!("curl -fsSL {} | {}", s.install_url, s.shell), "updated_at": s.updated_at,
            })
        }));
        if !activate::pending_journals(&self.layout)?.is_empty() {
            res.warnings.push("an interrupted operation needs recovery: run `w3bs repair`".into());
        }
        res.data = json!({ "packages": items });
        Ok(res)
    }

    pub fn info(&mut self, raw: &str) -> Result<OperationResult> {
        let mut res = OperationResult::new("info");
        let r: PackageRef = raw.parse().map_err(|e: w3bs_schema::identity::IdentityError| Error::usage("INVALID_PACKAGE", e.to_string()))?;
        let layout = self.layout.clone();
        match self.installed(&r)? {
            None => return Err(Error::package("NOT_INSTALLED", format!("{r} is not installed; try `w3bs search {r}`"))),
            Some(Installed::Native(p)) => {
                let c = self.read_state()?.unwrap();
                let links = db::links(c, &p.package_id)?;
                let files = db::owned_files(c, &p.package_id, &p.active_dir)?;
                res.data = json!({
                    "id": p.package_id, "kind": "native", "description": p.description, "version": p.active_version,
                    "channel": p.channel, "update_policy": p.update_policy, "pinned": p.pinned_version,
                    "previous_version": p.previous_version, "repository": p.repo_alias, "root_fingerprint": p.root_fingerprint,
                    "artifact_sha256": p.active_digest, "platform": p.platform, "origin": p.origin,
                    "install_dir": layout.data_dir.join(&p.active_dir),
                    "commands": links.iter().map(|l| json!({ "command": l.command, "path": l.path, "target": l.target })).collect::<Vec<_>>(),
                    "owned_files": files.len(), "installed_at": p.installed_at, "updated_at": p.updated_at,
                    "history": db::history(c, &p.package_id)?,
                });
            }
            Some(Installed::Script(s)) => {
                let c = self.read_state()?.unwrap();
                let signature = match db::release_signature(c, &s.package_id)? {
                    Some(sig) => json!({ "status": "signed", "version": sig.version, "fingerprint": sig.fingerprint, "installer_sha256": sig.installer_sha256, "verified_at": sig.verified_at }),
                    None => json!("unsigned"),
                };
                res.data = json!({
                    "id": s.package_id, "kind": "curl", "description": s.description, "repository": s.repo_alias,
                    "signature": signature,
                    "installer": format!("curl -fsSL {} | {}", s.install_url, s.shell), "installer_sha256": s.script_sha256,
                    "commands": s.commands, "owner": "publisher-installer",
                    "note": "installed by the publisher's own script; W3BS records it and re-runs the installer on update, but does not own, verify or roll back its files",
                    "installed_at": s.installed_at, "updated_at": s.updated_at, "history": db::history(c, &s.package_id)?,
                });
            }
        }
        Ok(res)
    }

    pub fn doctor(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("doctor");
        let layout = self.layout.clone();
        let mut issues: Vec<serde_json::Value> = vec![];
        let mut issue = |sev: &str, code: &str, msg: String| issues.push(json!({ "severity": sev, "code": code, "message": msg }));
        for j in activate::pending_journals(&layout)? {
            issue("error", "RECOVERY_REQUIRED", format!("interrupted operation {}; run `w3bs repair`", j.display()));
        }
        if let Err(e) = crate::layout::check_local_filesystem(&layout.data_dir) {
            issue("error", e.code, e.message);
        }
        let path_ok = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == layout.bin_dir));
        if !path_ok {
            issue("warning", "BIN_NOT_ON_PATH", format!("{} is not on PATH; add `export PATH=\"{}:$PATH\"` to your shell profile", layout.bin_dir.display(), layout.bin_dir.display()));
        }
        if let Some(c) = self.read_state()? {
            for r in db::repos(c)? {
                let t = layout.trust_dir(&r.repo_key);
                if !t.join("initial-root.json").exists() {
                    issue("error", "TRUST_STATE", format!("repository `{}` has no stored trust root", r.alias));
                }
                if let Ok(b) = std::fs::read(t.join("latest_known_time.json"))
                    && let Ok(ts) = serde_json::from_slice::<jiff::Timestamp>(&b)
                    && ts > jiff::Timestamp::now()
                {
                    issue("error", "CLOCK_BEHIND", format!("system clock is earlier than the last verified time for `{}` ({ts}); check the clock", r.alias));
                }
            }
            for p in db::packages(c)? {
                let pkg_dir = layout.package_dir(&p.package_key);
                let want = p.active_dir.strip_prefix(&format!("packages/{}/", p.package_key)).unwrap_or(&p.active_dir).to_owned();
                if activate::current_target(&pkg_dir).as_deref() != Some(want.as_str()) {
                    issue("error", "ACTIVE_POINTER", format!("{}: `current` does not point at the committed version {}", p.package_id, p.active_version));
                }
                let base = layout.data_dir.join(&p.active_dir);
                for (rel, sha) in db::owned_files(c, &p.package_id, &p.active_dir)? {
                    let f = base.join(&rel);
                    let ok = match sha.strip_prefix("symlink:") {
                        Some(t) => std::fs::read_link(&f).is_ok_and(|x| x.to_string_lossy() == t),
                        None => fsutil::sha256_file(&f).is_ok_and(|h| h == sha),
                    };
                    if !ok {
                        issue("error", "PAYLOAD_MODIFIED", format!("{}: {rel} is missing or modified", p.package_id));
                    }
                }
                for l in db::links(c, &p.package_id)? {
                    if !activate::link_is(std::path::Path::new(&l.path), &l.target) {
                        issue("warning", "LAUNCH_LINK", format!("{}: {} was removed or replaced by something else", p.package_id, l.path));
                    }
                }
                if let Some(reason) = db::revocation(c, &p.repo_key, &p.active_digest)? {
                    issue("warning", "REVOKED", format!("{} {} is revoked by its publisher: {reason}", p.package_id, p.active_version));
                }
            }
        }
        let errors = issues.iter().filter(|i| i["severity"] == "error").count();
        res.result = if errors > 0 { Overall::Failure } else { Overall::Ok };
        res.data = json!({ "issues": issues, "store": layout.data_dir, "bin_dir": layout.bin_dir, "manager_version": super::MANAGER_VERSION, "platform": w3bs_schema::host_platform() });
        Ok(res)
    }

    pub fn repair(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("repair");
        let pending = activate::pending_journals(&self.layout)?;
        if self.opts.dry_run {
            res.data = json!({ "pending": pending });
            return Ok(res);
        }
        // begin_mutation reconciles every journal deterministically.
        self.begin_mutation()?;
        res.data = json!({ "recovered": pending.len() });
        Ok(res)
    }

    pub fn gc(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("gc");
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let layout = self.layout.clone();
        let mut keep_dirs = BTreeSet::new();
        let mut keep_blobs = BTreeSet::new();
        if let Some(c) = self.read_state()? {
            for p in db::packages(c)? {
                keep_dirs.insert(layout.data_dir.join(&p.active_dir));
                keep_blobs.insert(p.active_digest.clone());
                if let (Some(d), Some(s)) = (&p.previous_dir, &p.previous_digest) {
                    keep_dirs.insert(layout.data_dir.join(d));
                    keep_blobs.insert(s.clone());
                }
            }
        }
        let mut doomed = vec![];
        if let Ok(rd) = std::fs::read_dir(layout.cache_dir.join("blobs/sha256")) {
            for e in rd.flatten() {
                if !keep_blobs.contains(&e.file_name().to_string_lossy().into_owned()) {
                    doomed.push(e.path());
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(layout.packages_dir()) {
            for pkg in rd.flatten() {
                if let Ok(vs) = std::fs::read_dir(pkg.path().join("versions")) {
                    for v in vs.flatten() {
                        if let Ok(ds) = std::fs::read_dir(v.path()) {
                            for d in ds.flatten() {
                                if !keep_dirs.contains(&d.path()) {
                                    doomed.push(d.path());
                                }
                            }
                        }
                    }
                }
                if let Ok(entries) = std::fs::read_dir(pkg.path()) {
                    for e in entries.flatten() {
                        if e.file_name().to_string_lossy().starts_with(".staging-") {
                            doomed.push(e.path());
                        }
                    }
                }
            }
        }
        if !self.opts.dry_run {
            for p in &doomed {
                let _ = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
            }
        }
        res.data = json!({ "removed": doomed, "dry_run": self.opts.dry_run });
        Ok(res)
    }

    /// Bootstrap: register the manager repository and install the manager through the
    /// normal verified engine. Used by `install.sh`; also a documented command.
    pub async fn self_install(&mut self, url: &str, fingerprint: Option<String>, root_file: Option<std::path::PathBuf>) -> Result<OperationResult> {
        let has_repo = self.read_state()?.map(|c| db::repo(c, "w3bs")).transpose()?.flatten().is_some();
        if !has_repo {
            self.repo_add(super::repos::RepoAdd { alias: "w3bs".into(), url: url.into(), root_file, root_fingerprint: fingerprint }).await?;
        }
        let mut r = self
            .install(InstallRequest { packages: vec![MANAGER_PACKAGE_ID.into()], repository: Some("w3bs".into()), origin: Some("curl".into()), ..Default::default() })
            .await?;
        r.operation = "self.install".into();
        Ok(r)
    }

    /// A newer published w3bs that this binary cannot install over itself, because it is not
    /// the managed one (baked into an image, cargo, Homebrew, a copied binary). A managed w3bs
    /// updates itself in `update`, so this is None for it, and on any lookup failure.
    pub async fn external_manager_notice(&mut self) -> Option<String> {
        if manager_is_self_managed(&self.layout) {
            return None;
        }
        let (latest, how) = self.newer_manager().await?;
        Some(format!("w3bs {latest} is available (you have {}): {how}", super::MANAGER_VERSION))
    }

    /// The newest published w3bs above this one, and how to get it from here.
    pub async fn newer_manager(&mut self) -> Option<(semver::Version, String)> {
        let current: semver::Version = super::MANAGER_VERSION.parse().ok()?;
        let latest = self.published_version(MANAGER_PACKAGE_ID, "w3bs", "stable", &current).await?;
        let how = if manager_is_self_managed(&self.layout) { "upgrade with `w3bs self update`".into() } else { external_upgrade_hint() };
        Some((latest, how))
    }

    pub async fn self_update(&mut self) -> Result<OperationResult> {
        if !manager_is_self_managed(&self.layout) {
            return Err(Error::package("SELF_UPDATE_EXTERNAL", external_owner_hint().unwrap_or_default()));
        }
        let mut r = self.update(UpdateRequest { packages: vec![MANAGER_PACKAGE_ID.into()], ..Default::default() }).await?;
        r.operation = "self.update".into();
        Ok(r)
    }

    pub fn self_remove(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("self.remove");
        if let Some(c) = self.read_state()? {
            let others: Vec<String> = db::packages(c)?
                .into_iter()
                .map(|p| p.package_id)
                .filter(|id| id != MANAGER_PACKAGE_ID)
                .chain(db::scripted_all(c)?.into_iter().map(|s| s.package_id))
                .collect();
            if !others.is_empty() {
                return Err(Error::package("PACKAGES_REMAIN", format!("remove managed applications first (`w3bs remove --all`): {}", others.join(", "))));
            }
        }
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let row = db::package(self.conn(), MANAGER_PACKAGE_ID)?;
        let Some(row) = row else {
            return Err(Error::package("SELF_REMOVE_EXTERNAL", external_owner_hint().unwrap_or_default()));
        };
        if self.opts.dry_run {
            res.data = json!({ "planned": "remove W3BS Package Manager", "store": self.layout.data_dir });
            return Ok(res);
        }
        self.approve("Remove W3BS Package Manager and its state? Trust history and receipts are deleted too.")?;
        for l in db::links(self.conn(), &row.package_id)? {
            if activate::link_is(std::path::Path::new(&l.path), &l.target) {
                let _ = std::fs::remove_file(&l.path);
            }
        }
        let layout = self.layout.clone();
        self.conn = None;
        for d in [&layout.data_dir, &layout.cache_dir, &layout.state_dir] {
            let _ = std::fs::remove_dir_all(d);
        }
        res.data = json!({ "removed": true, "kept": layout.config_dir });
        Ok(res)
    }
}
