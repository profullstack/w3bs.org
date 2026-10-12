//! Package CRUD: install (create), update, pin/unpin/rollback, remove (delete).

use super::activate::{self, Journal};
use super::{Engine, MANAGER_VERSION, checkpoint, txn_id};
use crate::db::{self, PackageRow, ScriptedRow};
use crate::error::{Error, Kind, Result};
use crate::fsutil;
use crate::repo::LoadedRepo;
use crate::{archive, now};
use semver::Version;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use w3bs_schema::manifest::{Artifact, ArtifactFormat, CatalogEntry, ChannelIndex, Manifest};
use w3bs_schema::policy::{Selection, select_latest, select_upgrade};
use w3bs_schema::receipt::Receipt;
use w3bs_schema::result::{OperationResult, PackageOutcome, Status};
use w3bs_schema::{PackageId, PackageRef, host_platform};

#[derive(Debug, Clone, Default)]
pub struct InstallRequest {
    pub packages: Vec<String>,
    pub manifest: Option<String>,
    pub repository: Option<String>,
    pub version: Option<Version>,
    pub channel: Option<String>,
    pub pin: bool,
    /// Diagnostic origin hint (`native` or `curl`); never affects ownership or security.
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateRequest {
    pub packages: Vec<String>,
    pub all: bool,
    pub version: Option<Version>,
    pub channel: Option<String>,
    pub allow_major: bool,
}

/// A fully verified, ready-to-apply native change.
pub(crate) struct Planned {
    pub id: PackageId,
    pub repo: Rc<LoadedRepo>,
    pub manifest: Manifest,
    pub artifact: Artifact,
    pub channel: String,
    pub from: Option<PackageRow>,
    pub pin: bool,
    pub origin: Option<String>,
}

/// A curl-catalog change: run the publisher's installer.
pub(crate) struct PlannedScript {
    pub entry: CatalogEntry,
    pub repo: Rc<LoadedRepo>,
    pub from: Option<ScriptedRow>,
}

pub(crate) enum Change {
    Native(Box<Planned>),
    Script(Box<PlannedScript>),
}

impl Change {
    fn id(&self) -> String {
        match self {
            Change::Native(p) => p.id.to_string(),
            Change::Script(s) => s.entry.id.to_string(),
        }
    }
}

fn outcome(id: &str, from: Option<&str>, to: Option<&str>, status: Status, reason: Option<String>) -> PackageOutcome {
    PackageOutcome { id: id.into(), from: from.map(Into::into), to: to.map(Into::into), status, reason, error: None }
}

fn failure(id: &str, from: Option<&str>, e: &Error, status: Status) -> PackageOutcome {
    PackageOutcome { id: id.into(), from: from.map(Into::into), to: None, status, reason: None, error: Some(e.info()) }
}

fn manager_satisfies(m: &Manifest) -> bool {
    match &m.requirements.manager {
        None => true,
        Some(req) => semver::VersionReq::parse(req).is_ok_and(|r| r.matches(&MANAGER_VERSION.parse().unwrap())),
    }
}

pub(crate) fn parse_refs(raw: &[String]) -> Result<Vec<PackageRef>> {
    let mut out: Vec<PackageRef> = vec![];
    for r in raw {
        let p: PackageRef = r.parse().map_err(|e: w3bs_schema::identity::IdentityError| Error::usage("INVALID_PACKAGE", e.to_string()))?;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// What a reference resolves to locally.
pub(crate) enum Installed {
    Native(PackageRow),
    Script(ScriptedRow),
}

impl Engine {
    /// Resolve a user reference against local state. Ambiguity is a usage error.
    pub(crate) fn installed(&mut self, r: &PackageRef) -> Result<Option<Installed>> {
        let Some(conn) = self.read_state()? else { return Ok(None) };
        let mut hits: Vec<Installed> = vec![];
        for p in db::packages(conn)? {
            if r.matches(&p.package_id.parse().unwrap()) {
                hits.push(Installed::Native(p));
            }
        }
        for s in db::scripted_all(conn)? {
            if r.matches(&s.package_id.parse().unwrap()) {
                hits.push(Installed::Script(s));
            }
        }
        match hits.len() {
            0 => Ok(None),
            1 => Ok(hits.pop()),
            _ => Err(Error::usage("AMBIGUOUS_PACKAGE", format!("`{r}` matches several installed packages; use the qualified name"))),
        }
    }

    /// Find a not-yet-installed package across configured, authenticated repositories.
    async fn resolve_new(&mut self, r: &PackageRef, only: Option<&str>) -> Result<(Rc<LoadedRepo>, Resolved)> {
        let aliases: Vec<String> = match only {
            Some(a) => vec![a.to_owned()],
            None => self.repo_aliases()?,
        };
        if aliases.is_empty() {
            return Err(Error::usage("NO_REPOSITORIES", "no repositories are configured; add one with `w3bs repo add <alias> <https-url>`"));
        }
        let mut found: Vec<(Rc<LoadedRepo>, Resolved)> = vec![];
        let mut unreachable: Vec<String> = vec![];
        for alias in aliases {
            let repo = match self.repo(&alias).await {
                Ok(r) => r,
                Err(e) if e.kind == Kind::Usage => return Err(e),
                Err(e) => {
                    unreachable.push(format!("{alias}: {}", e.message));
                    continue;
                }
            };
            let (index, catalog) = match (repo.package_index_or_empty().await, repo.catalog().await) {
                (Ok(i), Ok(c)) => (i, c),
                (Err(e), _) | (_, Err(e)) => {
                    unreachable.push(format!("{alias}: {}", e.message));
                    continue;
                }
            };
            for e in index.packages.into_iter().filter(|e| r.matches(&e.id)) {
                found.push((repo.clone(), Resolved::Native(e)));
            }
            for e in catalog.entries.into_iter().filter(|e| r.matches(&e.id)) {
                // A native package wins over a catalog entry for the same identity in one repository.
                if !found.iter().any(|(rr, x)| rr.alias == repo.alias && *x.id() == e.id) {
                    found.push((repo.clone(), Resolved::Script(e)));
                }
            }
        }
        match found.len() {
            1 => Ok(found.pop().unwrap()),
            0 if !unreachable.is_empty() => Err(Error::package(
                "REPOSITORY_UNAVAILABLE",
                format!("`{r}` was not found in the repositories that could be checked; unreachable: {}", unreachable.join("; ")),
            )
            .retryable()),
            0 => Err(Error::package("PACKAGE_NOT_FOUND", format!("no configured repository offers `{r}`; see `w3bs search`"))),
            _ => Err(Error::usage(
                "AMBIGUOUS_PACKAGE",
                format!(
                    "`{r}` is offered by several sources: {}. Use the qualified name and --repository <alias>",
                    found.iter().map(|(rr, x)| format!("{} from {} ({})", x.id(), rr.alias, rr.fingerprint)).collect::<Vec<_>>().join(", ")
                ),
            )),
        }
    }

    /// Verify everything about a release before any payload is downloaded.
    async fn plan_release(
        &mut self,
        repo: &Rc<LoadedRepo>,
        id: &PackageId,
        channel: &str,
        channel_path: &str,
        version: Option<&Version>,
        installed: Option<&Version>,
        allow_major: bool,
    ) -> Result<std::result::Result<Manifest, PackageOutcome>> {
        let ch: ChannelIndex = repo.channel(channel_path).await?;
        if ch.package_id != *id || ch.channel != channel {
            return Err(Error::package("CHANNEL_MISMATCH", format!("channel index `{channel_path}` does not describe {id} on `{channel}`")));
        }
        if !self.opts.dry_run {
            for rv in &ch.revoked {
                db::record_revocation(self.conn(), &repo.repo_key, &rv.sha256, &rv.reason)?;
            }
        }
        let prerelease_ok = channel != "stable";
        let versions: Vec<&Version> = ch.releases.iter().map(|r| &r.version).collect();
        let target: &Version = match (version, installed) {
            (Some(v), Some(cur)) if v < cur => {
                return Err(Error::usage("DOWNGRADE_REJECTED", format!("{id} {v} is older than installed {cur}; use `w3bs rollback` to return to the retained previous version")));
            }
            (Some(v), Some(cur)) if v == cur => return Ok(Err(outcome(&id.to_string(), Some(&cur.to_string()), None, Status::Current, None))),
            (Some(v), Some(cur)) if !allow_major && !w3bs_schema::policy::is_compatible(cur, v) => {
                return Ok(Err(outcome(&id.to_string(), Some(&cur.to_string()), Some(&v.to_string()), Status::Held, Some("major version: re-run with --allow-major".into()))));
            }
            (Some(v), _) => versions.iter().find(|x| **x == v).copied().ok_or_else(|| Error::package("VERSION_NOT_FOUND", format!("{id} {v} is not published on `{channel}`")))?,
            (None, Some(cur)) => match select_upgrade(cur, versions.iter().copied(), allow_major, prerelease_ok) {
                Selection::Current => return Ok(Err(outcome(&id.to_string(), Some(&cur.to_string()), None, Status::Current, None))),
                Selection::HeldMajor(v) => {
                    return Ok(Err(outcome(&id.to_string(), Some(&cur.to_string()), Some(&v.to_string()), Status::Held, Some("major version: re-run with --allow-major".into()))));
                }
                Selection::Upgrade(v) => v,
            },
            (None, None) => select_latest(versions.iter().copied(), prerelease_ok).ok_or_else(|| Error::package("NO_RELEASES", format!("{id} has no installable release on `{channel}`")))?,
        };
        let rel = ch.releases.iter().find(|r| &r.version == target).unwrap();
        let m = repo.manifest(&rel.manifest).await?;
        if m.package.id != *id || m.release.version != *target || m.release.channel != channel {
            return Err(Error::package("MANIFEST_MISMATCH", format!("manifest `{}` does not describe {id} {target} on `{channel}`", rel.manifest)));
        }
        Ok(Ok(m))
    }

    /// Platform, artifact binding, revocation, immutability and command-collision checks.
    fn check_artifact(&mut self, repo: &LoadedRepo, m: &Manifest, current: Option<&PackageRow>) -> Result<Artifact> {
        let platform = host_platform();
        let a = m
            .artifact_for(platform)
            .cloned()
            .ok_or_else(|| Error::package("UNSUPPORTED_PLATFORM", format!("{} {} has no artifact for {platform}", m.package.id, m.release.version)))?;
        if !manager_satisfies(m) {
            return Err(Error::package(
                "MANAGER_TOO_OLD",
                format!("{} {} needs W3BS Package Manager {}, this is {MANAGER_VERSION}; run `w3bs self update`", m.package.id, m.release.version, m.requirements.manager.as_deref().unwrap_or("")),
            ));
        }
        match repo.target_info(&a.target_path) {
            Some((len, sha)) if len == a.length && sha == a.sha256 => {}
            Some(_) => return Err(Error::package("ARTIFACT_MISMATCH", format!("signed target metadata for `{}` disagrees with the manifest", a.target_path))),
            None => return Err(Error::package("TARGET_NOT_FOUND", format!("artifact `{}` is not a signed target", a.target_path))),
        }
        if let Some(conn) = self.read_state()? {
            if let Some(reason) = db::revocation(conn, &repo.repo_key, &a.sha256)? {
                return Err(Error::package("REVOKED", format!("{} {} was revoked by its publisher: {reason}", m.package.id, m.release.version)));
            }
            if let Some(seen) = db::observed_digest(conn, &repo.repo_key, &m.package.id.to_string(), &m.release.version.to_string(), platform)?
                && seen != a.sha256
            {
                return Err(Error::package(
                    "IMMUTABILITY_VIOLATION",
                    format!("{} {} previously had artifact {seen} and now claims {}; a published version must never change", m.package.id, m.release.version, a.sha256),
                ));
            }
        }
        for cmd in a.commands.keys() {
            let path = self.layout.bin_dir.join(cmd);
            if std::fs::symlink_metadata(&path).is_ok() {
                let owner = match self.read_state()? {
                    Some(c) => db::link_owner(c, &path.to_string_lossy())?,
                    None => None,
                };
                if owner.as_deref() != Some(&m.package.id.to_string()) {
                    return Err(Error::package("COMMAND_CONFLICT", format!("{} already exists and is not managed by {}; W3BS will not overwrite it", path.display(), m.package.id)));
                }
            }
        }
        let _ = current;
        Ok(a)
    }

    // ---------------------------------------------------------------- install (create)

    pub async fn install(&mut self, req: InstallRequest) -> Result<OperationResult> {
        let mut res = OperationResult::new("install");
        if req.packages.is_empty() && req.manifest.is_none() {
            return Err(Error::usage("MISSING_PACKAGE", "install needs at least one package (or --manifest <https-url>)"));
        }
        if let Some(c) = &req.channel {
            w3bs_schema::manifest::validate_channel(c).map_err(|e| Error::usage("INVALID_CHANNEL", e))?;
        }
        // Validate every argument before touching state.
        let refs = parse_refs(&req.packages)?;
        if refs.len() > 1 && req.version.is_some() {
            return Err(Error::usage("VERSION_NEEDS_ONE_PACKAGE", "--version selects a release of exactly one package"));
        }
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let txn = txn_id();
        res.transaction_id = Some(txn.clone());

        let mut changes: Vec<Change> = vec![];
        if let Some(url) = req.manifest.clone() {
            match self.plan_manifest_install(&url, &req).await {
                Ok(Ok(c)) => changes.push(c),
                Ok(Err(o)) => res.packages.push(o),
                Err(e) if matches!(e.kind, Kind::Usage | Kind::Approval | Kind::Preflight) => return Err(e),
                Err(e) => res.packages.push(failure(&url, None, &e, Status::Failed)),
            }
        }
        for r in &refs {
            match self.plan_install(r, &req).await {
                // Two spellings of one identity (`atlas`, `tools.example/atlas`) plan once.
                Ok(Ok(c)) if changes.iter().any(|x| x.id() == c.id()) => {}
                Ok(Err(o)) if res.packages.iter().any(|x| x.id == o.id) => {}
                Ok(Ok(c)) => changes.push(c),
                Ok(Err(o)) => res.packages.push(o),
                Err(e) if matches!(e.kind, Kind::Usage | Kind::Approval | Kind::Preflight) => return Err(e),
                Err(e) => res.packages.push(failure(&r.to_string(), None, &e, Status::Failed)),
            }
        }
        self.apply_changes(changes, &txn, &mut res, "install").await?;
        res.settle();
        Ok(res)
    }

    async fn plan_install(&mut self, r: &PackageRef, req: &InstallRequest) -> Result<std::result::Result<Change, PackageOutcome>> {
        // Idempotency: an installed identity is never silently updated, downgraded or moved.
        match self.installed(r)? {
            Some(Installed::Native(row)) => {
                let same_version = req.version.as_ref().is_none_or(|v| v.to_string() == row.active_version);
                let same_channel = req.channel.as_ref().is_none_or(|c| *c == row.channel);
                let same_repo = req.repository.as_ref().is_none_or(|a| *a == row.repo_alias);
                if same_version && same_channel && same_repo {
                    if req.pin && row.pinned_version.as_deref() != Some(&row.active_version) && !self.opts.dry_run {
                        self.set_pin(&row, Some(row.active_version.clone()))?;
                        return Ok(Err(outcome(&row.package_id, Some(&row.active_version), None, Status::Pinned, Some("already installed; pinned".into()))));
                    }
                    return Ok(Err(outcome(&row.package_id, Some(&row.active_version), None, Status::Current, Some("already installed; use `w3bs update` to upgrade".into()))));
                }
                return Ok(Err(failure(
                    &row.package_id,
                    Some(&row.active_version),
                    &Error::package("INSTALL_CONFLICT", format!("{} {} is installed from `{}` on `{}`; install will not change version, channel or source. Use `w3bs update` or `w3bs rollback`", row.package_id, row.active_version, row.repo_alias, row.channel)),
                    Status::Failed,
                )));
            }
            Some(Installed::Script(row)) => {
                return Ok(Err(outcome(&row.package_id, None, None, Status::Current, Some("already installed; use `w3bs update` to re-run its installer".into()))));
            }
            None => {}
        }
        let (repo, resolved) = self.resolve_new(r, req.repository.as_deref()).await?;
        match resolved {
            Resolved::Script(entry) => {
                if req.version.is_some() || req.channel.is_some() || req.pin {
                    return Err(Error::usage("SCRIPT_NO_VERSIONS", format!("{} installs through its publisher's curl installer; --version, --channel and --pin do not apply", entry.id)));
                }
                Ok(Ok(Change::Script(Box::new(PlannedScript { entry, repo, from: None }))))
            }
            Resolved::Native(entry) => {
                let channel = req.channel.clone().unwrap_or_else(|| "stable".into());
                let path = entry
                    .channels
                    .get(&channel)
                    .cloned()
                    .ok_or_else(|| Error::package("CHANNEL_NOT_FOUND", format!("{} has no `{channel}` channel (available: {})", entry.id, entry.channels.keys().cloned().collect::<Vec<_>>().join(", "))))?;
                let m = match self.plan_release(&repo, &entry.id, &channel, &path, req.version.as_ref(), None, false).await? {
                    Ok(m) => m,
                    Err(o) => return Ok(Err(o)),
                };
                let artifact = self.check_artifact(&repo, &m, None)?;
                Ok(Ok(Change::Native(Box::new(Planned { id: entry.id, repo, manifest: m, artifact, channel, from: None, pin: req.pin, origin: req.origin.clone() }))))
            }
        }
    }

    async fn plan_manifest_install(&mut self, url: &str, req: &InstallRequest) -> Result<std::result::Result<Change, PackageOutcome>> {
        if !req.packages.is_empty() || req.version.is_some() || req.channel.is_some() {
            return Err(Error::usage("MANIFEST_EXCLUSIVE", "--manifest cannot be combined with package names, --version or --channel"));
        }
        let parsed = url::Url::parse(url).map_err(|e| Error::usage("INVALID_URL", format!("{url}: {e}")))?;
        if !crate::transport::url_allowed(&parsed, self.layout.dev_transport) {
            return Err(Error::usage("INSECURE_URL", "--manifest must be an https:// URL"));
        }
        let rows = match self.read_state()? {
            Some(c) => db::repos(c)?,
            None => vec![],
        };
        let candidates: Vec<(String, String)> = rows
            .iter()
            .filter(|r| req.repository.as_ref().is_none_or(|a| *a == r.alias))
            .filter_map(|r| {
                let base = format!("{}targets/", crate::repo::normalize_base(&r.base_url, self.layout.dev_transport).ok()?);
                url.strip_prefix(&base).map(|t| (r.alias.clone(), t.to_owned()))
            })
            .collect();
        let (alias, target) = match candidates.len() {
            1 => candidates.into_iter().next().unwrap(),
            0 => {
                return Err(Error::usage(
                    "MANIFEST_REPOSITORY",
                    "the manifest URL is not under the targets of a configured repository; add the repository first (`w3bs repo add`) and pass --repository <alias>",
                ));
            }
            _ => return Err(Error::usage("MANIFEST_REPOSITORY", "several repositories match this URL; pass --repository <alias>")),
        };
        let repo = self.repo(&alias).await?;
        let m = repo.manifest(&target).await?;
        let id = m.package.id.clone();
        let r = PackageRef::Qualified(id.clone());
        if self.installed(&r)?.is_some() {
            return Ok(Err(outcome(&id.to_string(), None, None, Status::Current, Some("already installed; use `w3bs update`".into()))));
        }
        // Revocations come from the channel index of the same repository.
        let index = repo.package_index_or_empty().await?;
        if let Some(e) = index.packages.iter().find(|e| e.id == id)
            && let Some(path) = e.channels.get(&m.release.channel)
        {
            let ch = repo.channel(path).await?;
            if !self.opts.dry_run {
                for rv in &ch.revoked {
                    db::record_revocation(self.conn(), &repo.repo_key, &rv.sha256, &rv.reason)?;
                }
            }
        }
        let artifact = self.check_artifact(&repo, &m, None)?;
        let channel = m.release.channel.clone();
        Ok(Ok(Change::Native(Box::new(Planned { id, repo, manifest: m, artifact, channel, from: None, pin: req.pin, origin: req.origin.clone() }))))
    }

    /// Show the plan, get approval, apply each change independently.
    pub(crate) async fn apply_changes(&mut self, changes: Vec<Change>, txn: &str, res: &mut OperationResult, op: &str) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let mut lines = vec![];
        for c in &changes {
            lines.push(match c {
                Change::Native(p) => format!(
                    "  {op} {} {} -> {} ({}, from {} {})",
                    p.id,
                    p.from.as_ref().map(|f| f.active_version.as_str()).unwrap_or("-"),
                    p.manifest.release.version,
                    p.channel,
                    p.repo.alias,
                    p.repo.fingerprint
                ),
                Change::Script(s) => format!(
                    "  {op} {} by running its publisher's installer: {}\n    (listed by {}; the script itself is not verified by W3BS and cannot be rolled back)",
                    s.entry.id,
                    s.entry.install.command(),
                    s.repo.alias
                ),
            });
        }
        if self.opts.dry_run {
            for c in &changes {
                let (from, to) = match c {
                    Change::Native(p) => (p.from.as_ref().map(|f| f.active_version.clone()), Some(p.manifest.release.version.to_string())),
                    Change::Script(_) => (None, None),
                };
                res.packages.push(outcome(&c.id(), from.as_deref(), to.as_deref(), Status::Planned, None));
            }
            return Ok(());
        }
        self.approve(&format!("W3BS will:\n{}\nProceed?", lines.join("\n")))?;
        for c in changes {
            let id = c.id();
            let r = match &c {
                Change::Native(p) => self.apply_native(p, txn).await.map(|_| {
                    let st = if p.from.is_some() { Status::Updated } else { Status::Installed };
                    outcome(&id, p.from.as_ref().map(|f| f.active_version.as_str()), Some(&p.manifest.release.version.to_string()), st, None)
                }),
                Change::Script(s) => self.apply_script(s, txn).await.map(|_| {
                    let st = if s.from.is_some() { Status::Updated } else { Status::Installed };
                    outcome(&id, None, None, st, Some("ran the publisher's curl installer".into()))
                }),
            };
            match r {
                Ok(o) => {
                    self.progress(&format!("{:?} {id}", o.status).to_lowercase());
                    res.packages.push(o);
                }
                Err(e) => {
                    let from = match &c {
                        Change::Native(p) => p.from.as_ref().map(|f| f.active_version.clone()),
                        Change::Script(_) => None,
                    };
                    res.packages.push(failure(&id, from.as_deref(), &e, Status::Failed));
                }
            }
        }
        Ok(())
    }

    async fn fetch_artifact(&self, p: &Planned) -> Result<Vec<u8>> {
        let a = &p.artifact;
        let blob = self.layout.blob_path(&a.sha256);
        if let Ok(md) = std::fs::metadata(&blob)
            && md.len() == a.length
            && fsutil::sha256_file(&blob)? == a.sha256
        {
            return std::fs::read(&blob).map_err(Into::into);
        }
        self.progress(&format!("downloading {} {} ({} bytes)", p.id, p.manifest.release.version, a.length));
        let bytes = p.repo.read_target(&a.target_path, a.length).await?;
        if bytes.len() as u64 != a.length || fsutil::sha256_bytes(&bytes) != a.sha256 {
            return Err(Error::package("DIGEST_MISMATCH", format!("downloaded artifact for {} does not match its signed digest", p.id)));
        }
        fsutil::write_durable(&blob, &bytes)?;
        Ok(bytes)
    }

    async fn apply_native(&mut self, p: &Planned, txn: &str) -> Result<()> {
        let key = p.id.storage_key();
        let pkg_dir = self.layout.package_dir(&key);
        let bytes = self.fetch_artifact(p).await?;
        std::fs::create_dir_all(&pkg_dir)?;
        let staging = format!(".staging-{txn}");
        let stage_dir = pkg_dir.join(&staging);
        let _ = std::fs::remove_dir_all(&stage_dir);
        std::fs::create_dir_all(&stage_dir)?;
        let staged = (|| -> Result<Vec<archive::Entry>> {
            let entries = match p.artifact.format {
                ArtifactFormat::TarGz => archive::extract_tar_gz(&bytes, &stage_dir, p.artifact.unpacked_size_limit)?,
                ArtifactFormat::Binary => archive::place_binary(&bytes, &stage_dir, p.artifact.commands.values().next().unwrap())?,
            };
            for rel in p.artifact.commands.values() {
                archive::resolve_command(&stage_dir, &entries, rel)?;
            }
            fsutil::sync_tree(&stage_dir)?;
            Ok(entries)
        })();
        let entries = match staged {
            Ok(e) => e,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&stage_dir);
                if p.from.is_none() {
                    let _ = std::fs::remove_dir(&pkg_dir);
                }
                return Err(e);
            }
        };
        let version = p.manifest.release.version.to_string();
        let new_target = format!("versions/{version}/{}", p.artifact.sha256);
        let new_dir_rel = format!("packages/{key}/{new_target}");
        let links = activate::launch_links(&self.layout, &key, &p.artifact.commands);
        let j = Journal {
            transaction_id: txn.to_owned(),
            operation: if p.from.is_some() { "update".into() } else { "install".into() },
            package_id: p.id.to_string(),
            package_key: key.clone(),
            old_target: activate::current_target(&pkg_dir),
            new_target: new_target.clone(),
            staging: Some(staging),
            new_links: links.clone(),
        };
        if let Err(e) = activate::promote_and_activate(self, &j) {
            super::activate::recover_all(self)?;
            return Err(e);
        }

        // Commit.
        let platform = host_platform().to_owned();
        let t = now();
        let row = PackageRow {
            package_id: p.id.to_string(),
            package_key: key.clone(),
            description: p.manifest.package.description.clone(),
            repo_alias: p.repo.alias.clone(),
            repo_key: p.repo.repo_key.clone(),
            root_fingerprint: p.repo.fingerprint.clone(),
            channel: p.channel.clone(),
            update_policy: "compatible".into(),
            pinned_version: match &p.from {
                Some(f) => f.pinned_version.clone(),
                None if p.pin => Some(version.clone()),
                None => None,
            },
            active_version: version.clone(),
            active_digest: p.artifact.sha256.clone(),
            active_dir: new_dir_rel.clone(),
            previous_version: p.from.as_ref().map(|f| f.active_version.clone()),
            previous_digest: p.from.as_ref().map(|f| f.active_digest.clone()),
            previous_dir: p.from.as_ref().map(|f| f.active_dir.clone()),
            platform: platform.clone(),
            commands: p.artifact.commands.keys().cloned().collect(),
            installed_at: p.from.as_ref().map(|f| f.installed_at.clone()).unwrap_or_else(|| t.clone()),
            updated_at: t,
            origin: p.from.as_ref().and_then(|f| f.origin.clone()).or_else(|| p.origin.clone()),
            transaction_id: txn.to_owned(),
        };
        let dropped_dir = p.from.as_ref().and_then(|f| f.previous_dir.clone()).filter(|d| *d != new_dir_rel);
        {
            let conn = self.conn();
            let tx = conn.unchecked_transaction()?;
            db::upsert_package(&tx, &row)?;
            tx.execute(
                "INSERT OR REPLACE INTO versions(package_id, dir, version, sha256, commands) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![row.package_id, new_dir_rel, version, row.active_digest, serde_json::to_string(&p.artifact.commands).unwrap()],
            )?;
            tx.execute("DELETE FROM owned_files WHERE package_id=?1 AND dir=?2", rusqlite::params![row.package_id, new_dir_rel])?;
            for (rel, sha) in &entries {
                tx.execute("INSERT INTO owned_files(package_id, dir, rel, sha256) VALUES (?1,?2,?3,?4)", rusqlite::params![row.package_id, new_dir_rel, rel, sha])?;
            }
            tx.execute("DELETE FROM launch_links WHERE package_id=?1", [&row.package_id])?;
            for ((path, target), cmd) in links.iter().zip(p.artifact.commands.keys()) {
                tx.execute("INSERT INTO launch_links(package_id, command, path, target) VALUES (?1,?2,?3,?4)", rusqlite::params![row.package_id, cmd, path, target])?;
            }
            if let Some(d) = &dropped_dir {
                db::delete_version(&tx, &row.package_id, d)?;
            }
            db::record_observed(&tx, &row.repo_key, &row.package_id, &version, &platform, &row.active_digest)?;
            db::record_history(&tx, &row.package_id, &j.operation, row.previous_version.as_deref(), Some(&version), txn)?;
            tx.commit()?;
        }
        checkpoint("committed");
        self.write_receipt(&row)?;
        activate::clear_journal(&self.layout, &j);
        // Retention: keep active + one previous complete version.
        if let Some(d) = dropped_dir {
            let _ = std::fs::remove_dir_all(self.layout.data_dir.join(d));
        }
        Ok(())
    }

    pub(crate) fn write_receipt(&self, row: &PackageRow) -> Result<()> {
        let r = Receipt {
            schema_version: "1.0".into(),
            package_id: row.package_id.parse().unwrap(),
            owner: "w3bs".into(),
            installed_version: row.active_version.clone(),
            platform: row.platform.clone(),
            repository_id: row.repo_alias.clone(),
            root_fingerprint: row.root_fingerprint.clone(),
            artifact_sha256: row.active_digest.clone(),
            installed_at: row.installed_at.clone(),
            channel: row.channel.clone(),
            update_policy: row.update_policy.clone(),
            pinned_version: row.pinned_version.clone(),
            previous_version: row.previous_version.clone(),
            commands: row.commands.clone(),
            transaction_id: row.transaction_id.clone(),
            origin: row.origin.clone(),
        };
        fsutil::write_durable(&self.layout.receipts_dir().join(format!("{}.json", row.package_key)), &serde_json::to_vec_pretty(&r).unwrap())
    }

    /// Run a publisher's curl installer. The script is fetched over HTTPS by W3BS itself
    /// (no shell `curl`), saved, hashed and run with the documented interpreter.
    async fn apply_script(&mut self, s: &PlannedScript, txn: &str) -> Result<()> {
        use futures::TryStreamExt;
        use tough::Transport;
        let url = url::Url::parse(&s.entry.install.url).map_err(|e| Error::package("INVALID_INSTALLER", e.to_string()))?;
        if url.scheme() != "https" && !self.layout.dev_transport {
            return Err(Error::package("INSECURE_INSTALLER", "installer URL must be https://"));
        }
        self.progress(&format!("fetching installer {}", s.entry.install.url));
        let t = crate::transport::W3bsTransport::new(self.layout.dev_transport);
        let mut stream = t.fetch(url.clone()).await.map_err(|e| Error::package("INSTALLER_UNAVAILABLE", format!("{url}: {e}")).retryable())?;
        let mut script = Vec::new();
        while let Some(chunk) = stream.try_next().await.map_err(|e| Error::package("INSTALLER_UNAVAILABLE", format!("{url}: {e}")).retryable())? {
            script.extend_from_slice(&chunk);
            if script.len() > 4 * 1024 * 1024 {
                return Err(Error::package("INSTALLER_TOO_LARGE", "installer script exceeds 4 MiB"));
            }
        }
        if script.is_empty() {
            return Err(Error::package("INSTALLER_EMPTY", format!("{url} returned an empty installer")));
        }
        let sha = fsutil::sha256_bytes(&script);
        let path = self.layout.cache_dir.join("installers").join(format!("{sha}.sh"));
        fsutil::write_durable(&path, &script)?;
        let shell = if s.entry.install.shell == "bash" { "bash" } else { "sh" };
        self.progress(&format!("running {} installer ({shell}, sha256 {})", s.entry.id, &sha[..12]));
        let status = std::process::Command::new(shell)
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .env("W3BS", "1")
            .status()
            .map_err(|e| Error::package("INSTALLER_FAILED", format!("cannot run {shell}: {e}")))?;
        if !status.success() {
            return Err(Error::package("INSTALLER_FAILED", format!("{} installer exited with {status}", s.entry.id)));
        }
        let t = now();
        let row = ScriptedRow {
            package_id: s.entry.id.to_string(),
            description: s.entry.description.clone(),
            repo_alias: s.repo.alias.clone(),
            repo_key: s.repo.repo_key.clone(),
            install_url: s.entry.install.url.clone(),
            shell: shell.to_owned(),
            script_sha256: sha,
            commands: s.entry.commands.clone(),
            installed_at: s.from.as_ref().map(|f| f.installed_at.clone()).unwrap_or_else(|| t.clone()),
            updated_at: t,
            transaction_id: txn.to_owned(),
        };
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        db::upsert_scripted(&tx, &row)?;
        db::record_history(&tx, &row.package_id, if s.from.is_some() { "update" } else { "install" }, None, None, txn)?;
        tx.commit()?;
        Ok(())
    }

    // ---------------------------------------------------------------- update

    /// Plan updates for the selected packages. Shared by `update` and `outdated`.
    pub(crate) async fn plan_updates(&mut self, req: &UpdateRequest, res: &mut OperationResult) -> Result<Vec<Change>> {
        if req.all && !req.packages.is_empty() {
            return Err(Error::usage("ALL_WITH_PACKAGES", "--all cannot be combined with package names"));
        }
        let refs = parse_refs(&req.packages)?;
        if (req.version.is_some() || req.channel.is_some()) && refs.len() != 1 {
            return Err(Error::usage("SELECTOR_NEEDS_ONE_PACKAGE", "--version and --channel apply to exactly one named package"));
        }
        let mut native: Vec<PackageRow> = vec![];
        let mut scripted: Vec<ScriptedRow> = vec![];
        if refs.is_empty() {
            if let Some(c) = self.read_state()? {
                native = db::packages(c)?;
                scripted = db::scripted_all(c)?;
            }
        } else {
            for r in &refs {
                match self.installed(r)? {
                    Some(Installed::Native(p)) => native.push(p),
                    Some(Installed::Script(s)) => scripted.push(s),
                    None => res.packages.push(failure(&r.to_string(), None, &Error::package("NOT_INSTALLED", format!("{r} is not installed")), Status::Failed)),
                }
            }
        }
        // The manager goes first so a newer manager plans everything else.
        native.sort_by_key(|p| !p.package_id.parse::<PackageId>().is_ok_and(|i| i.is_manager()));
        let mut changes = vec![];
        for row in native {
            let id: PackageId = row.package_id.parse().unwrap();
            if id.is_manager() && !crate::engine::maint::manager_is_self_managed(&self.layout) {
                res.packages.push(outcome(&row.package_id, Some(&row.active_version), None, Status::Skipped, Some("this w3bs binary is not the managed one".into())));
                continue;
            }
            let installed: Version = row.active_version.parse().map_err(|_| Error::preflight("STATE", "invalid installed version"))?;
            let channel = req.channel.clone().unwrap_or_else(|| row.channel.clone());
            let r = async {
                let repo = self.repo(&row.repo_alias).await?;
                if repo.repo_key != row.repo_key {
                    return Err(Error::package("TRUST_CHANGED", format!("repository `{}` no longer has the trust root {} was installed from", row.repo_alias, row.package_id)));
                }
                let index = repo.package_index_or_empty().await?;
                let entry = index.packages.iter().find(|e| e.id == id).ok_or_else(|| Error::package("PACKAGE_WITHDRAWN", format!("{id} is no longer listed by `{}`", row.repo_alias)))?;
                let path = entry.channels.get(&channel).cloned().ok_or_else(|| Error::package("CHANNEL_NOT_FOUND", format!("{id} has no `{channel}` channel")))?;
                // Known revocation of the installed artifact is surfaced even when pinned.
                let planned = self.plan_release(&repo, &id, &channel, &path, req.version.as_ref(), Some(&installed), req.allow_major).await?;
                Ok((repo, planned))
            }
            .await;
            let (repo, planned) = match r {
                Ok(x) => x,
                Err(e) if e.kind == Kind::Usage => return Err(e),
                Err(e) => {
                    let st = if e.code == "REPOSITORY_UNAVAILABLE" { Status::Unknown } else { Status::Failed };
                    let mut o = failure(&row.package_id, Some(&row.active_version), &e, st);
                    if st == Status::Unknown {
                        o.status = Status::Failed;
                    }
                    res.packages.push(o);
                    continue;
                }
            };
            if let Some(reason) = db::revocation(self.conn_or_open()?, &row.repo_key, &row.active_digest)? {
                res.warnings.push(format!("{} {} is revoked by its publisher: {reason}", row.package_id, row.active_version));
            }
            let m = match planned {
                Err(o) => {
                    res.packages.push(o);
                    continue;
                }
                Ok(m) => m,
            };
            if let Some(pin) = &row.pinned_version {
                res.packages.push(outcome(&row.package_id, Some(&row.active_version), Some(&m.release.version.to_string()), Status::Pinned, Some(format!("pinned to {pin}; `w3bs unpin` to allow updates"))));
                continue;
            }
            match self.check_artifact(&repo, &m, Some(&row)) {
                Ok(artifact) => changes.push(Change::Native(Box::new(Planned {
                    id,
                    repo,
                    artifact,
                    channel: m.release.channel.clone(),
                    manifest: m,
                    from: Some(row),
                    pin: false,
                    origin: None,
                }))),
                Err(e) => {
                    let st = if matches!(e.code, "REVOKED" | "MANAGER_TOO_OLD" | "UNSUPPORTED_PLATFORM") { Status::Blocked } else { Status::Failed };
                    res.packages.push(failure(&row.package_id, Some(&row.active_version), &e, st));
                }
            }
        }
        if req.version.is_some() && !scripted.is_empty() {
            return Err(Error::usage("SCRIPT_NO_VERSIONS", "curl-installed tools have no selectable versions"));
        }
        for s in scripted {
            let id: PackageId = s.package_id.parse().unwrap();
            let r = async {
                let repo = self.repo(&s.repo_alias).await?;
                if repo.repo_key != s.repo_key {
                    return Err(Error::package("TRUST_CHANGED", format!("repository `{}` changed trust root", s.repo_alias)));
                }
                let cat = repo.catalog().await?;
                let entry = cat.entries.into_iter().find(|e| e.id == id).ok_or_else(|| Error::package("PACKAGE_WITHDRAWN", format!("{id} is no longer listed by `{}`", s.repo_alias)))?;
                Ok((repo, entry))
            }
            .await;
            match r {
                Ok((repo, entry)) => changes.push(Change::Script(Box::new(PlannedScript { entry, repo, from: Some(s) }))),
                Err(e) => res.packages.push(failure(&s.package_id, None, &e, Status::Failed)),
            }
        }
        Ok(changes)
    }

    fn conn_or_open(&mut self) -> Result<&rusqlite::Connection> {
        self.read_state()?.ok_or_else(|| Error::preflight("STATE", "no state database"))
    }

    pub async fn update(&mut self, req: UpdateRequest) -> Result<OperationResult> {
        let mut res = OperationResult::new("update");
        parse_refs(&req.packages)?;
        if req.all && !req.packages.is_empty() {
            return Err(Error::usage("ALL_WITH_PACKAGES", "--all cannot be combined with package names"));
        }
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let txn = txn_id();
        res.transaction_id = Some(txn.clone());
        let mut changes = self.plan_updates(&req, &mut res).await?;
        // Manager handoff: apply the manager alone, then the caller re-executes the new binary.
        let manager_first = matches!(changes.first(), Some(Change::Native(p)) if p.id.is_manager()) && changes.len() > 1 && !self.opts.dry_run;
        if manager_first {
            if std::env::var("W3BS_HANDOFF").is_ok() {
                return Err(Error::preflight("MANAGER_HANDOFF_LOOP", "the updated manager still plans a manager update; refusing to loop"));
            }
            changes.truncate(1);
            res.data = serde_json::json!({ "handoff": true });
        }
        self.apply_changes(changes, &txn, &mut res, "update").await?;
        if manager_first && res.packages.iter().any(|p| p.status == Status::Failed) {
            res.data = serde_json::Value::Null;
        }
        res.settle();
        Ok(res)
    }

    pub async fn outdated(&mut self) -> Result<OperationResult> {
        let mut res = OperationResult::new("outdated");
        self.begin_read()?;
        let changes = self.plan_updates(&UpdateRequest { all: true, ..Default::default() }, &mut res).await?;
        for c in changes {
            match c {
                Change::Native(p) => res.packages.push(outcome(
                    &p.id.to_string(),
                    p.from.as_ref().map(|f| f.active_version.as_str()),
                    Some(&p.manifest.release.version.to_string()),
                    Status::Planned,
                    Some("eligible".into()),
                )),
                Change::Script(s) => res.packages.push(outcome(&s.entry.id.to_string(), None, None, Status::Unknown, Some("curl-installed; `w3bs update` re-runs its installer".into()))),
            }
        }
        res.settle();
        Ok(res)
    }

    // ---------------------------------------------------------------- pin / unpin / rollback

    fn native_row(&mut self, raw: &str) -> Result<PackageRow> {
        let r: PackageRef = raw.parse().map_err(|e: w3bs_schema::identity::IdentityError| Error::usage("INVALID_PACKAGE", e.to_string()))?;
        match self.installed(&r)? {
            Some(Installed::Native(p)) => Ok(p),
            Some(Installed::Script(s)) => Err(Error::package("SCRIPT_UNSUPPORTED", format!("{} was installed by its publisher's curl script; pins and rollback need a signed W3BS package", s.package_id))),
            None => Err(Error::package("NOT_INSTALLED", format!("{r} is not installed"))),
        }
    }

    fn set_pin(&self, row: &PackageRow, pin: Option<String>) -> Result<()> {
        let mut r = row.clone();
        r.pinned_version = pin;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        db::upsert_package(&tx, &r)?;
        db::record_history(&tx, &r.package_id, if r.pinned_version.is_some() { "pin" } else { "unpin" }, Some(&r.active_version), None, &txn_id())?;
        tx.commit()?;
        self.write_receipt(&r)
    }

    pub fn pin(&mut self, raw: &str, pin: bool) -> Result<OperationResult> {
        let op = if pin { "pin" } else { "unpin" };
        let mut res = OperationResult::new(op);
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let row = self.native_row(raw)?;
        let target = pin.then(|| row.active_version.clone());
        let status = if pin { Status::Pinned } else { Status::Unpinned };
        if row.pinned_version == target {
            res.packages.push(outcome(&row.package_id, Some(&row.active_version), None, status, Some("no change".into())));
        } else if self.opts.dry_run {
            res.packages.push(outcome(&row.package_id, Some(&row.active_version), None, Status::Planned, Some(op.into())));
        } else {
            self.set_pin(&row, target)?;
            res.packages.push(outcome(&row.package_id, Some(&row.active_version), None, status, None));
        }
        Ok(res)
    }

    pub fn rollback(&mut self, raw: &str) -> Result<OperationResult> {
        let mut res = OperationResult::new("rollback");
        res.warnings.push("rollback restores application files only; configuration, databases, running processes and migrations are not rolled back".into());
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let row = self.native_row(raw)?;
        let (Some(prev_v), Some(prev_dir), Some(prev_sha)) = (row.previous_version.clone(), row.previous_dir.clone(), row.previous_digest.clone()) else {
            return Err(Error::package("NO_PREVIOUS_VERSION", format!("{} has no retained previous version", row.package_id)));
        };
        if let Some(reason) = db::revocation(self.conn(), &row.repo_key, &prev_sha)? {
            return Err(Error::package("REVOKED", format!("{} {prev_v} is revoked by its publisher ({reason}); refusing to activate it", row.package_id)));
        }
        res.warnings.push("rollback uses the locally retained artifact; current remote revocation status was not checked".into());
        let ver = db::version_row(self.conn(), &row.package_id, &prev_dir)?.ok_or_else(|| Error::preflight("STATE", "retained version is not recorded"))?;
        // Verify the retained files before activating them.
        let base = self.layout.data_dir.join(&prev_dir);
        for (rel, sha) in db::owned_files(self.conn(), &row.package_id, &prev_dir)? {
            let p = base.join(&rel);
            let ok = if let Some(t) = sha.strip_prefix("symlink:") {
                std::fs::read_link(&p).is_ok_and(|x| x.to_string_lossy() == t)
            } else {
                fsutil::sha256_file(&p).is_ok_and(|h| h == sha)
            };
            if !ok {
                return Err(Error::package("RETAINED_VERSION_CORRUPT", format!("retained file {rel} of {} {prev_v} does not match its recorded digest", row.package_id)));
            }
        }
        if self.opts.dry_run {
            res.packages.push(outcome(&row.package_id, Some(&row.active_version), Some(&prev_v), Status::Planned, None));
            return Ok(res);
        }
        self.approve(&format!(
            "Roll back {} {} -> {prev_v}? Only application files change; data and configuration are not rolled back. The package will be pinned to {prev_v}.",
            row.package_id, row.active_version
        ))?;
        let key = row.package_key.clone();
        let new_target = prev_dir.strip_prefix(&format!("packages/{key}/")).unwrap().to_owned();
        let links = activate::launch_links(&self.layout, &key, &ver.commands);
        let txn = txn_id();
        let j = Journal {
            transaction_id: txn.clone(),
            operation: "rollback".into(),
            package_id: row.package_id.clone(),
            package_key: key.clone(),
            old_target: activate::current_target(&self.layout.package_dir(&key)),
            new_target,
            staging: None,
            new_links: links.clone(),
        };
        if let Err(e) = activate::promote_and_activate(self, &j) {
            activate::recover_all(self)?;
            return Err(e);
        }
        let mut r = row.clone();
        r.active_version = prev_v.clone();
        r.active_digest = prev_sha;
        r.active_dir = prev_dir;
        r.previous_version = Some(row.active_version.clone());
        r.previous_digest = Some(row.active_digest.clone());
        r.previous_dir = Some(row.active_dir.clone());
        r.pinned_version = Some(prev_v.clone());
        r.commands = ver.commands.keys().cloned().collect();
        r.updated_at = now();
        r.transaction_id = txn.clone();
        {
            let conn = self.conn();
            let tx = conn.unchecked_transaction()?;
            db::upsert_package(&tx, &r)?;
            tx.execute("DELETE FROM launch_links WHERE package_id=?1", [&r.package_id])?;
            for ((path, target), cmd) in links.iter().zip(ver.commands.keys()) {
                tx.execute("INSERT INTO launch_links(package_id, command, path, target) VALUES (?1,?2,?3,?4)", rusqlite::params![r.package_id, cmd, path, target])?;
            }
            db::record_history(&tx, &r.package_id, "rollback", Some(&row.active_version), Some(&prev_v), &txn)?;
            tx.commit()?;
        }
        checkpoint("committed");
        self.write_receipt(&r)?;
        activate::clear_journal(&self.layout, &j);
        res.transaction_id = Some(txn);
        res.packages.push(outcome(&r.package_id, Some(&row.active_version), Some(&prev_v), Status::RolledBack, Some(format!("pinned to {prev_v}"))));
        Ok(res)
    }

    // ---------------------------------------------------------------- remove (delete)

    pub fn remove(&mut self, raw: &[String], all: bool) -> Result<OperationResult> {
        let mut res = OperationResult::new("remove");
        if all && !raw.is_empty() {
            return Err(Error::usage("ALL_WITH_PACKAGES", "--all cannot be combined with package names"));
        }
        if !all && raw.is_empty() {
            return Err(Error::usage("MISSING_PACKAGE", "remove needs package names, or --all to remove every managed application"));
        }
        parse_refs(raw)?;
        if self.opts.dry_run { self.begin_read()? } else { self.begin_mutation()? }
        let mut targets: Vec<Installed> = vec![];
        if all {
            if let Some(c) = self.read_state()? {
                targets.extend(db::packages(c)?.into_iter().filter(|p| !p.package_id.parse::<PackageId>().unwrap().is_manager()).map(Installed::Native));
                targets.extend(db::scripted_all(c)?.into_iter().map(Installed::Script));
            }
        } else {
            for r in parse_refs(raw)? {
                match self.installed(&r)? {
                    Some(Installed::Native(p)) if p.package_id.parse::<PackageId>().unwrap().is_manager() => {
                        return Err(Error::usage("REMOVE_MANAGER", "use `w3bs self remove` to remove W3BS Package Manager"));
                    }
                    Some(t) => targets.push(t),
                    None => res.packages.push(failure(&r.to_string(), None, &Error::package("NOT_INSTALLED", format!("{r} is not installed")), Status::Failed)),
                }
            }
        }
        if targets.is_empty() {
            res.settle();
            return Ok(res);
        }
        let names: Vec<String> = targets
            .iter()
            .map(|t| match t {
                Installed::Native(p) => format!("  remove {} {} (application data and configuration are kept)", p.package_id, p.active_version),
                Installed::Script(s) => format!("  forget {} (installed by its own curl script; W3BS does not own its files)", s.package_id),
            })
            .collect();
        if self.opts.dry_run {
            for t in &targets {
                let (id, v) = match t {
                    Installed::Native(p) => (p.package_id.clone(), Some(p.active_version.clone())),
                    Installed::Script(s) => (s.package_id.clone(), None),
                };
                res.packages.push(outcome(&id, v.as_deref(), None, Status::Planned, None));
            }
            return Ok(res);
        }
        self.approve(&format!("W3BS will:\n{}\nProceed?", names.join("\n")))?;
        let txn = txn_id();
        res.transaction_id = Some(txn.clone());
        for t in targets {
            match t {
                Installed::Native(p) => {
                    let o = self.remove_native(&p, &txn);
                    res.packages.push(match o {
                        Ok(conflicts) if conflicts.is_empty() => outcome(&p.package_id, Some(&p.active_version), None, Status::Removed, None),
                        Ok(conflicts) => outcome(&p.package_id, Some(&p.active_version), None, Status::Removed, Some(format!("left in place (changed by someone else): {}", conflicts.join(", ")))),
                        Err(e) => failure(&p.package_id, Some(&p.active_version), &e, Status::Failed),
                    });
                }
                Installed::Script(s) => {
                    let r = (|| -> Result<()> {
                        let conn = self.conn();
                        let tx = conn.unchecked_transaction()?;
                        db::delete_scripted(&tx, &s.package_id)?;
                        db::record_history(&tx, &s.package_id, "remove", None, None, &txn)?;
                        tx.commit()?;
                        Ok(())
                    })();
                    res.packages.push(match r {
                        Ok(()) => outcome(
                            &s.package_id,
                            None,
                            None,
                            Status::Removed,
                            Some(format!("unregistered; its files ({}) were installed by the publisher's script, use the publisher's uninstall instructions", s.commands.join(", "))),
                        ),
                        Err(e) => failure(&s.package_id, None, &e, Status::Failed),
                    });
                }
            }
        }
        res.settle();
        Ok(res)
    }

    fn remove_native(&mut self, p: &PackageRow, txn: &str) -> Result<Vec<String>> {
        let mut conflicts = vec![];
        for l in db::links(self.conn(), &p.package_id)? {
            let path = Path::new(&l.path);
            if activate::link_is(path, &l.target) {
                std::fs::remove_file(path)?;
            } else if std::fs::symlink_metadata(path).is_ok() {
                conflicts.push(l.path.clone());
            }
        }
        {
            let conn = self.conn();
            let tx = conn.unchecked_transaction()?;
            db::delete_package(&tx, &p.package_id)?;
            db::record_history(&tx, &p.package_id, "remove", Some(&p.active_version), None, txn)?;
            tx.commit()?;
        }
        // The package directory is W3BS's own store, never a manifest-supplied path.
        let _ = std::fs::remove_dir_all(self.layout.package_dir(&p.package_key));
        let _ = std::fs::remove_file(self.layout.receipts_dir().join(format!("{}.json", p.package_key)));
        Ok(conflicts)
    }
}

pub(crate) enum Resolved {
    Native(w3bs_schema::manifest::IndexEntry),
    Script(CatalogEntry),
}

impl Resolved {
    fn id(&self) -> &PackageId {
        match self {
            Resolved::Native(e) => &e.id,
            Resolved::Script(e) => &e.id,
        }
    }
}

#[allow(dead_code)]
fn _assert_map(_: &BTreeMap<String, String>) {}
