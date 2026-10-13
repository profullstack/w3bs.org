//! Publisher tooling: a repository workspace is
//!
//! ```text
//! <workspace>/
//!   keys/{root,targets,snapshot,timestamp}.pk8   ed25519 PKCS#8, 0600, never published
//!   staging/                                      the target tree you edit
//!     manifests/<name>/<version>.json            immutable release ledger
//!     artifacts/<name>/<version>/<file>
//!     catalog/index.json                          optional curl catalog
//!     revocations.json                            optional
//!   repo/                                         publish this directory over HTTPS
//!     metadata/{1.root.json,root.json,targets.json,snapshot.json,timestamp.json}
//!     targets/...                                 copy of staging + generated indexes
//! ```

use anyhow::{Context, Result, anyhow, bail};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::Ed25519KeyPair;
use semver::Version;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use tough::editor::RepositoryEditor;
use tough::editor::signed::SignedRole;
use tough::key_source::{KeySource, LocalKeySource};
use tough::schema::{KeyHolder, RoleKeys, RoleType, Root, Target};
use tough::sign::parse_keypair;
use w3bs_schema::manifest::{
    ArtifactFormat, CATALOG_INDEX_TARGET, CatalogEntry, CatalogIndex, ChannelIndex, ChannelRelease, IndexEntry, Manifest, PACKAGE_INDEX_TARGET, PackageIndex, Revocation, SCHEMA_VERSION,
};
use w3bs_schema::PackageId;

pub const ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];

pub struct Workspace {
    pub dir: PathBuf,
}

fn role_type(r: &str) -> RoleType {
    match r {
        "root" => RoleType::Root,
        "targets" => RoleType::Targets,
        "snapshot" => RoleType::Snapshot,
        _ => RoleType::Timestamp,
    }
}

fn days(n: i64) -> jiff::Timestamp {
    jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_hours(24 * n)).unwrap()
}

fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn fingerprint(root_bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(root_bytes))
}

fn write_private(p: &Path, b: &[u8]) -> Result<()> {
    std::fs::write(p, b)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl Workspace {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn keys_dir(&self) -> PathBuf {
        std::env::var_os("W3BS_PUBLISH_KEYS").map(PathBuf::from).unwrap_or_else(|| self.dir.join("keys"))
    }
    pub fn staging(&self) -> PathBuf {
        self.dir.join("staging")
    }
    pub fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }
    pub fn root_path(&self) -> PathBuf {
        self.repo().join("metadata/1.root.json")
    }

    fn key_sources(&self) -> Result<Vec<Box<dyn KeySource>>> {
        ROLES
            .iter()
            .map(|r| {
                let p = self.keys_dir().join(format!("{r}.pk8"));
                if !p.exists() {
                    bail!("missing signing key {}", p.display());
                }
                Ok(Box::new(LocalKeySource { path: p }) as Box<dyn KeySource>)
            })
            .collect()
    }

    /// Generate one ed25519 key per role and a signed version-1 root.
    pub async fn init(&self, root_expires_days: i64) -> Result<String> {
        if self.root_path().exists() {
            bail!("{} already exists; this workspace is initialized", self.root_path().display());
        }
        std::fs::create_dir_all(self.keys_dir())?;
        std::fs::create_dir_all(self.staging())?;
        std::fs::create_dir_all(self.repo().join("metadata"))?;
        std::fs::create_dir_all(self.repo().join("targets"))?;
        let rng = SystemRandom::new();
        let mut keys = HashMap::new();
        let mut roles = HashMap::new();
        for r in ROLES {
            let p = self.keys_dir().join(format!("{r}.pk8"));
            if !p.exists() {
                let doc = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| anyhow!("key generation failed"))?;
                write_private(&p, doc.as_ref())?;
            }
            let pair = parse_keypair(&std::fs::read(&p)?).map_err(|e| anyhow!("{}: {e}", p.display()))?;
            let key = tough::sign::Sign::tuf_key(&pair);
            let id = key.key_id()?;
            keys.insert(id.clone(), key);
            roles.insert(role_type(r), RoleKeys { keyids: vec![id], threshold: NonZeroU64::new(1).unwrap(), _extra: HashMap::new() });
        }
        let root = Root {
            spec_version: "1.0.0".into(),
            consistent_snapshot: false,
            version: NonZeroU64::new(1).unwrap(),
            expires: days(root_expires_days),
            keys,
            roles,
            _extra: HashMap::new(),
        };
        let holder = KeyHolder::Root(root.clone());
        let signed = SignedRole::new(root, &holder, &self.key_sources()?, &rng).await?;
        std::fs::write(self.root_path(), signed.buffer())?;
        std::fs::write(self.repo().join("metadata/root.json"), signed.buffer())?;
        Ok(fingerprint(signed.buffer()))
    }

    pub fn fingerprint(&self) -> Result<String> {
        Ok(fingerprint(&std::fs::read(self.root_path()).context("workspace is not initialized")?))
    }

    /// Add one immutable release: copy artifacts into staging and write its manifest.
    pub fn add_release(&self, spec: ReleaseSpec) -> Result<PathBuf> {
        let name = spec.id.name().to_owned();
        let version = spec.version.to_string();
        let mut artifacts = vec![];
        for (platform, file) in &spec.artifacts {
            let bytes = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
            let fname = file.file_name().unwrap().to_string_lossy().into_owned();
            let target_path = format!("artifacts/{name}/{version}/{fname}");
            let dest = self.staging().join(&target_path);
            if dest.exists() && std::fs::read(&dest)? != bytes {
                bail!("{target_path} already exists with different bytes; a published version is immutable, publish a new version");
            }
            std::fs::create_dir_all(dest.parent().unwrap())?;
            std::fs::write(&dest, &bytes)?;
            let unpacked = match spec.format {
                ArtifactFormat::Binary => bytes.len() as u64,
                ArtifactFormat::TarGz => (bytes.len() as u64).saturating_mul(10).max(64 * 1024 * 1024),
            };
            artifacts.push(w3bs_schema::manifest::Artifact {
                platform: platform.clone(),
                target_path,
                format: spec.format,
                length: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
                unpacked_size_limit: unpacked,
                commands: spec.commands.clone(),
            });
        }
        let m = Manifest {
            schema_version: SCHEMA_VERSION.into(),
            package: w3bs_schema::manifest::PackageInfo { id: spec.id.clone(), name: name.clone(), description: spec.description, license: spec.license },
            release: w3bs_schema::manifest::ReleaseInfo {
                version: spec.version.clone(),
                channel: spec.channel,
                published_at: jiff::Timestamp::now().round(jiff::Unit::Second)?.to_string(),
            },
            requirements: w3bs_schema::manifest::Requirements { manager: spec.manager_requirement, capabilities: vec![] },
            artifacts,
            extensions: BTreeMap::new(),
        };
        m.validate()?;
        let path = self.staging().join(format!("manifests/{name}/{version}.json"));
        if path.exists() {
            let old: Manifest = serde_json::from_slice(&std::fs::read(&path)?)?;
            if old.artifacts.iter().map(|a| (&a.platform, &a.sha256)).collect::<Vec<_>>() != m.artifacts.iter().map(|a| (&a.platform, &a.sha256)).collect::<Vec<_>>() {
                bail!("{} {version} is already published with different artifacts; versions are immutable", spec.id);
            }
            return Ok(path);
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, serde_json::to_vec_pretty(&m)?)?;
        Ok(path)
    }

    pub fn set_catalog(&self, entries: Vec<CatalogEntry>) -> Result<usize> {
        for e in &entries {
            e.validate()?;
        }
        let idx = CatalogIndex { schema_version: SCHEMA_VERSION.into(), entries };
        let p = self.staging().join(CATALOG_INDEX_TARGET);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_vec_pretty(&idx)?)?;
        Ok(idx.entries.len())
    }

    pub fn revoke(&self, id: &PackageId, version: &Version, reason: &str) -> Result<usize> {
        let m: Manifest = serde_json::from_slice(&std::fs::read(self.staging().join(format!("manifests/{}/{version}.json", id.name()))).context("no such release")?)?;
        let p = self.staging().join("revocations.json");
        let mut all: BTreeMap<String, Vec<Revocation>> = if p.exists() { serde_json::from_slice(&std::fs::read(&p)?)? } else { BTreeMap::new() };
        let list = all.entry(id.to_string()).or_default();
        for a in &m.artifacts {
            if !list.iter().any(|r| r.sha256 == a.sha256) {
                list.push(Revocation { sha256: a.sha256.clone(), reason: reason.into() });
            }
        }
        let n = list.len();
        std::fs::write(&p, serde_json::to_vec_pretty(&all)?)?;
        Ok(n)
    }

    /// Regenerate indexes from the release ledger.
    fn generate_indexes(&self) -> Result<()> {
        let mut by_pkg: BTreeMap<PackageId, Vec<Manifest>> = BTreeMap::new();
        let mdir = self.staging().join("manifests");
        if mdir.exists() {
            for e in walkdir::WalkDir::new(&mdir).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
                let m = Manifest::parse(&std::fs::read(e.path())?).with_context(|| format!("invalid manifest {}", e.path().display()))?;
                by_pkg.entry(m.package.id.clone()).or_default().push(m);
            }
        }
        let revs: BTreeMap<String, Vec<Revocation>> = match std::fs::read(self.staging().join("revocations.json")) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(_) => BTreeMap::new(),
        };
        let mut index = PackageIndex { schema_version: SCHEMA_VERSION.into(), packages: vec![] };
        for (id, mut ms) in by_pkg {
            ms.sort_by(|a, b| a.release.version.cmp(&b.release.version));
            let mut channels: BTreeMap<String, Vec<ChannelRelease>> = BTreeMap::new();
            for m in &ms {
                channels.entry(m.release.channel.clone()).or_default().push(ChannelRelease {
                    version: m.release.version.clone(),
                    manifest: format!("manifests/{}/{}.json", id.name(), m.release.version),
                });
            }
            let mut paths = BTreeMap::new();
            for (ch, releases) in channels {
                let path = format!("channels/{}/{ch}.json", id.name());
                let ci = ChannelIndex { schema_version: SCHEMA_VERSION.into(), package_id: id.clone(), channel: ch.clone(), releases, revoked: revs.get(&id.to_string()).cloned().unwrap_or_default() };
                let p = self.staging().join(&path);
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::write(&p, serde_json::to_vec_pretty(&ci)?)?;
                paths.insert(ch, path);
            }
            let last = ms.last().unwrap();
            index.packages.push(IndexEntry { id: id.clone(), name: id.name().into(), description: last.package.description.clone(), channels: paths });
        }
        let p = self.staging().join(PACKAGE_INDEX_TARGET);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_vec_pretty(&index)?)?;
        Ok(())
    }

    fn previous_version(&self, role: &str) -> u64 {
        std::fs::read(self.repo().join(format!("metadata/{role}.json")))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["signed"]["version"].as_u64())
            .unwrap_or(0)
    }

    /// Sign the staging tree into `repo/`. Targets are written before metadata, and
    /// timestamp last, so a client never sees metadata naming a file that is not there yet.
    pub async fn build(&self, expires: Expiry) -> Result<BuildReport> {
        self.generate_indexes()?;
        let mut editor = RepositoryEditor::new(self.root_path()).await?;
        let mut names = vec![];
        for e in walkdir::WalkDir::new(self.staging()).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
            let rel = e.path().strip_prefix(self.staging())?.to_string_lossy().into_owned();
            if rel == "revocations.json" {
                continue;
            }
            let target = Target::from_path(e.path()).await?;
            editor.add_target(rel.as_str(), target)?;
            names.push((rel, e.path().to_path_buf()));
        }
        let next = |r: &str| NonZeroU64::new(self.previous_version(r) + 1).unwrap();
        editor
            .targets_version(next("targets"))?
            .targets_expires(days(expires.targets))?
            .snapshot_version(next("snapshot"))
            .snapshot_expires(days(expires.snapshot))
            .timestamp_version(next("timestamp"))
            .timestamp_expires(days(expires.timestamp));
        let signed = editor.sign(&self.key_sources()?).await?;
        let tdir = self.repo().join("targets");
        std::fs::create_dir_all(&tdir)?;
        for (rel, src) in &names {
            let dest = tdir.join(rel);
            std::fs::create_dir_all(dest.parent().unwrap())?;
            std::fs::copy(src, &dest)?;
        }
        let tmp = self.dir.join(".metadata-out");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        signed.write(&tmp).await?;
        let mdir = self.repo().join("metadata");
        for f in ["targets.json", "snapshot.json", "timestamp.json"] {
            std::fs::rename(tmp.join(f), mdir.join(f)).with_context(|| format!("install {f}"))?;
        }
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(BuildReport { targets: names.len(), timestamp_version: self.previous_version("timestamp"), fingerprint: self.fingerprint()? })
    }
}

pub struct ReleaseSpec {
    pub id: PackageId,
    pub version: Version,
    pub channel: String,
    pub description: String,
    pub license: String,
    pub format: ArtifactFormat,
    pub artifacts: Vec<(String, PathBuf)>,
    pub commands: BTreeMap<String, String>,
    pub manager_requirement: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Expiry {
    pub targets: i64,
    pub snapshot: i64,
    pub timestamp: i64,
}

impl Default for Expiry {
    fn default() -> Self {
        Self { targets: 365, snapshot: 90, timestamp: 30 }
    }
}

pub struct BuildReport {
    pub targets: usize,
    pub timestamp_version: u64,
    pub fingerprint: String,
}

/// Parse a pasted `curl ... <url> ... | sh|bash` command into an installer reference.
pub fn parse_curl_command(cmd: &str) -> Option<w3bs_schema::manifest::CurlInstall> {
    let (left, right) = cmd.split_once('|')?;
    let shell = right.split_whitespace().next()?;
    if !matches!(shell, "sh" | "bash") || !left.trim_start().starts_with("curl") {
        return None;
    }
    let url = left.split_whitespace().find(|w| w.starts_with("https://"))?;
    Some(w3bs_schema::manifest::CurlInstall { url: url.into(), shell: shell.into() })
}

pub const INSTALLER_TEMPLATE: &str = include_str!("../../../bootstrap/install.sh");

/// Generate a publisher's `install.sh`: it bootstraps (or reuses) W3BS Package Manager and
/// installs one package through the shared engine. No `eval`; options are forwarded as argv.
pub fn render_installer(opts: &InstallerOptions) -> String {
    INSTALLER_TEMPLATE
        .replace("@@PACKAGE@@", opts.package.as_deref().unwrap_or(""))
        .replace("@@REPO_ALIAS@@", &opts.repo_alias)
        .replace("@@REPO_URL@@", &opts.repo_url)
        .replace("@@REPO_FINGERPRINT@@", &opts.repo_fingerprint)
        .replace("@@MANAGER_REPO_URL@@", &opts.manager_repo_url)
        .replace("@@MANAGER_FINGERPRINT@@", &opts.manager_fingerprint)
        .replace("@@MANAGER_DOWNLOAD_BASE@@", &opts.manager_download_base)
        .replace("@@MANAGER_VERSION@@", &opts.manager_version)
        .replace("@@MANAGER_SHA256S@@", &opts.manager_sha256s.iter().map(|(p, s)| format!("{p}={s}")).collect::<Vec<_>>().join(" "))
}

pub struct InstallerOptions {
    pub package: Option<String>,
    pub repo_alias: String,
    pub repo_url: String,
    pub repo_fingerprint: String,
    pub manager_repo_url: String,
    pub manager_fingerprint: String,
    pub manager_download_base: String,
    pub manager_version: String,
    pub manager_sha256s: Vec<(String, String)>,
}
