//! Repository CRUD and search.

use super::Engine;
use crate::db;
use crate::error::{Error, Kind, Result};
use crate::repo;
use serde_json::json;
use w3bs_schema::result::{OperationResult, Overall};

pub struct RepoAdd {
    pub alias: String,
    pub url: String,
    /// Operator-provisioned expected initial root metadata (headless trust).
    pub root_file: Option<std::path::PathBuf>,
    /// Operator-provisioned expected root fingerprint (`sha256:<hex>`).
    pub root_fingerprint: Option<String>,
}

fn validate_alias(a: &str) -> Result<()> {
    let ok = !a.is_empty() && a.len() <= 40 && a.as_bytes()[0].is_ascii_alphanumeric() && a.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok { Ok(()) } else { Err(Error::usage("INVALID_ALIAS", format!("repository alias `{a}` must be 1-40 chars of a-z, 0-9, `-`, `_`"))) }
}

impl Engine {
    pub async fn repo_add(&mut self, req: RepoAdd) -> Result<OperationResult> {
        let mut res = OperationResult::new("repo.add");
        validate_alias(&req.alias)?;
        let base = repo::normalize_base(&req.url, self.layout.dev_transport)?;
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        if let Some(c) = self.read_state()?
            && let Some(existing) = db::repo(c, &req.alias)?
        {
            if existing.base_url == base.as_str() {
                res.data = json!({ "alias": existing.alias, "url": existing.base_url, "root_fingerprint": existing.root_fingerprint, "status": "already configured" });
                return Ok(res);
            }
            return Err(Error::usage("ALIAS_EXISTS", format!("repository alias `{}` already points at {}; remove it first", req.alias, existing.base_url)));
        }
        let (root_bytes, provisioned) = match &req.root_file {
            Some(p) => (std::fs::read(p).map_err(|e| Error::usage("ROOT_FILE", format!("cannot read {}: {e}", p.display())))?, true),
            None => (repo::fetch_initial_root(&base, self.layout.dev_transport).await?, false),
        };
        repo::parse_root(&root_bytes)?;
        let fp = repo::fingerprint(&root_bytes);
        let fp_matches = match &req.root_fingerprint {
            Some(want) if *want == fp => true,
            Some(want) => {
                return Err(Error::preflight("ROOT_FINGERPRINT_MISMATCH", format!("repository root fingerprint is {fp}, expected {want}; refusing to trust it")));
            }
            None => false,
        };
        let key = repo::repo_key(&root_bytes);
        if let Some(c) = self.read_state()?
            && let Some(other) = db::repos(c)?.into_iter().find(|r| r.repo_key == key)
        {
            return Err(Error::usage("REPOSITORY_EXISTS", format!("this trust root is already configured as `{}`", other.alias)));
        }
        res.data = json!({ "alias": req.alias, "url": base.as_str(), "root_fingerprint": fp, "repository_key": key });
        if self.opts.dry_run {
            res.warnings.push("dry run: repository not added".into());
            return Ok(res);
        }
        self.approve_trust(
            provisioned || fp_matches,
            &format!(
                "Trust repository `{}`?\n  URL:         {}\n  Fingerprint: {fp}\nThis is first-use trust: it pins this signing root for future updates. It is not proof of who operates the repository.\nTrust it?",
                req.alias, base
            ),
        )?;
        repo::store_initial_root(&self.layout, &key, &root_bytes)?;
        let t = crate::now();
        self.conn().execute(
            "INSERT INTO repositories(alias, repo_key, base_url, root_fingerprint, added_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![req.alias, key, base.as_str(), fp, t],
        )?;
        self.conn().execute(
            "INSERT OR IGNORE INTO trust_history(repo_key, base_url, initial_fingerprint, first_trusted_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![key, base.as_str(), fp, t],
        )?;
        // Verify the whole chain now so a broken repository is caught at registration.
        match self.repo(&req.alias).await {
            Ok(r) => {
                self.conn().execute("UPDATE repositories SET refreshed_at=?1 WHERE alias=?2", rusqlite::params![crate::now(), req.alias])?;
                res.data["root_version"] = json!(r.root_version());
            }
            Err(e) => {
                self.conn().execute("DELETE FROM repositories WHERE alias=?1", [&req.alias])?;
                return Err(e);
            }
        }
        Ok(res)
    }

    pub fn repo_list(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("repo.list");
        let rows = match self.read_state()? {
            Some(c) => db::repos(c)?,
            None => vec![],
        };
        res.data = json!({ "repositories": rows.iter().map(|r| json!({
            "alias": r.alias, "url": r.base_url, "root_fingerprint": r.root_fingerprint,
            "added_at": r.added_at, "refreshed_at": r.refreshed_at,
        })).collect::<Vec<_>>() });
        Ok(res)
    }

    pub fn repo_info(&mut self, alias: &str) -> Result<OperationResult> {
        let mut res = OperationResult::new("repo.info");
        let layout = self.layout.clone();
        let c = self.read_state()?.ok_or_else(|| Error::usage("UNKNOWN_REPOSITORY", format!("no repository named `{alias}`")))?;
        let r = db::repo(c, alias)?.ok_or_else(|| Error::usage("UNKNOWN_REPOSITORY", format!("no repository named `{alias}`")))?;
        let pkgs: Vec<String> = db::packages(c)?.into_iter().filter(|p| p.repo_alias == alias).map(|p| p.package_id).chain(db::scripted_all(c)?.into_iter().filter(|s| s.repo_alias == alias).map(|s| s.package_id)).collect();
        let trusted_root_version = std::fs::read(layout.trust_dir(&r.repo_key).join("root.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["signed"]["version"].as_u64());
        res.data = json!({
            "alias": r.alias, "url": r.base_url, "root_fingerprint": r.root_fingerprint, "repository_key": r.repo_key,
            "trusted_root_version": trusted_root_version, "added_at": r.added_at, "refreshed_at": r.refreshed_at, "packages": pkgs,
        });
        Ok(res)
    }

    pub async fn repo_update(&mut self, alias: Option<&str>) -> Result<OperationResult> {
        let mut res = OperationResult::new("repo.update");
        if !self.opts.dry_run {
            self.begin_mutation()?;
        }
        let aliases = match alias {
            Some(a) => vec![a.to_owned()],
            None => self.repo_aliases()?,
        };
        let mut out = vec![];
        let mut failed = 0;
        for a in aliases {
            let r = async {
                let r = self.repo(&a).await?;
                let n = r.package_index_or_empty().await?.packages.len();
                let c = r.catalog().await?.entries.len();
                Ok::<_, Error>((r.root_version(), n, c))
            }
            .await;
            match r {
                Ok((rv, n, c)) => {
                    if !self.opts.dry_run {
                        self.conn().execute("UPDATE repositories SET refreshed_at=?1 WHERE alias=?2", rusqlite::params![crate::now(), a])?;
                    }
                    out.push(json!({ "alias": a, "result": "ok", "root_version": rv, "packages": n, "catalog_entries": c }));
                }
                Err(e) if e.kind == Kind::Usage => return Err(e),
                Err(e) => {
                    failed += 1;
                    out.push(json!({ "alias": a, "result": "failed", "error": e.info() }));
                }
            }
        }
        res.result = match (failed, out.len()) {
            (0, _) => Overall::Ok,
            (f, n) if f == n => Overall::Failure,
            _ => Overall::PartialFailure,
        };
        res.data = json!({ "repositories": out });
        Ok(res)
    }

    pub fn repo_remove(&mut self, alias: &str) -> Result<OperationResult> {
        let mut res = OperationResult::new("repo.remove");
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let c = self.read_state()?.ok_or_else(|| Error::usage("UNKNOWN_REPOSITORY", format!("no repository named `{alias}`")))?;
        db::repo(c, alias)?.ok_or_else(|| Error::usage("UNKNOWN_REPOSITORY", format!("no repository named `{alias}`")))?;
        let using: Vec<String> = db::packages(c)?.into_iter().filter(|p| p.repo_alias == alias).map(|p| p.package_id).chain(db::scripted_all(c)?.into_iter().filter(|s| s.repo_alias == alias).map(|s| s.package_id)).collect();
        if !using.is_empty() {
            return Err(Error::package("REPOSITORY_IN_USE", format!("`{alias}` is still used by: {}; remove those packages first", using.join(", "))));
        }
        if self.opts.dry_run {
            res.data = json!({ "alias": alias, "planned": "remove" });
            return Ok(res);
        }
        self.approve(&format!("Remove repository `{alias}`? Its trust history is kept so it cannot be re-added as a different identity."))?;
        self.conn().execute("DELETE FROM repositories WHERE alias=?1", [alias])?;
        res.data = json!({ "alias": alias, "removed": true });
        Ok(res)
    }

    /// Search authenticated indexes and catalogs. Unreachable sources make the result
    /// explicitly incomplete rather than a definitive "not found".
    pub async fn search(&mut self, query: &str, only: Option<&str>) -> Result<OperationResult> {
        let mut res = OperationResult::new("search");
        let q = query.to_lowercase();
        let aliases = match only {
            Some(a) => vec![a.to_owned()],
            None => self.repo_aliases()?,
        };
        let mut hits = vec![];
        let mut unreachable = vec![];
        for a in aliases {
            let r = match self.repo(&a).await {
                Ok(r) => r,
                Err(e) if e.kind == Kind::Usage => return Err(e),
                Err(e) => {
                    unreachable.push(json!({ "alias": a, "error": e.info() }));
                    continue;
                }
            };
            let matches = |id: &str, name: &str, desc: &str| q.is_empty() || id.to_lowercase().contains(&q) || name.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q);
            match r.package_index_or_empty().await {
                Ok(i) => {
                    for e in i.packages.iter().filter(|e| matches(&e.id.to_string(), &e.name, &e.description)) {
                        hits.push(json!({ "id": e.id, "name": e.name, "description": e.description, "repository": a, "kind": "native", "channels": e.channels.keys().collect::<Vec<_>>() }));
                    }
                }
                Err(e) => unreachable.push(json!({ "alias": a, "error": e.info() })),
            }
            match r.catalog().await {
                Ok(c) => {
                    for e in c.entries.iter().filter(|e| matches(&e.id.to_string(), &e.name, &e.description)) {
                        hits.push(json!({ "id": e.id, "name": e.name, "title": e.title, "vendor": e.vendor, "homepage": e.homepage, "description": e.description, "repository": a, "kind": "curl", "install": e.install.command(), "agentic": e.agentic, "commands": e.commands }));
                    }
                }
                Err(e) => unreachable.push(json!({ "alias": a, "error": e.info() })),
            }
        }
        if !unreachable.is_empty() {
            res.warnings.push("search is incomplete: some repositories could not be checked".into());
        }
        res.data = json!({ "query": query, "complete": unreachable.is_empty(), "results": hits, "unreachable": unreachable });
        Ok(res)
    }
}
