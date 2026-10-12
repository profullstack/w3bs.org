//! Publisher repositories: TUF trust roots and authenticated target reads.
//!
//! Each repository is its own trust domain. Its identity (`repo_key`) is derived from the
//! root metadata the user first trusted, and its datastore (latest trusted root, timestamp
//! and snapshot) persists under `trust/<repo_key>/` so version counters never go backwards,
//! even across `repo remove` + `repo add`.

use crate::error::{Error, Result};
use crate::layout::Layout;
use crate::transport::{W3bsTransport, url_allowed};
use futures::TryStreamExt;
use sha2::{Digest, Sha256};
use tough::schema::{Root, Signed};
use tough::{IntoVec, Repository, RepositoryLoader, TargetName, Transport};
use url::Url;
use w3bs_schema::manifest::{CATALOG_INDEX_TARGET, CatalogIndex, ChannelIndex, Manifest, PACKAGE_INDEX_TARGET, PackageIndex};

/// Upper bound on small JSON targets (index, channel, manifest).
const MAX_DOCUMENT: u64 = 4 * 1024 * 1024;

pub fn fingerprint(root_bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(root_bytes)))
}

pub fn repo_key(root_bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(root_bytes))[..32].to_owned()
}

/// Normalize a repository base URL: scheme rules, trailing slash, no credentials/query.
pub fn normalize_base(url: &str, dev: bool) -> Result<Url> {
    let mut u = Url::parse(url).map_err(|e| Error::usage("INVALID_URL", format!("invalid repository URL `{url}`: {e}")))?;
    if !url_allowed(&u, dev) {
        return Err(Error::usage(
            "INSECURE_URL",
            format!("repository URL must use https:// (got `{url}`); file:// and loopback http:// are development-only"),
        ));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(Error::usage("CREDENTIALS_IN_URL", "repository URLs must not embed credentials"));
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err(Error::usage("INVALID_URL", "repository URLs must not have a query or fragment"));
    }
    if !u.path().ends_with('/') {
        let p = format!("{}/", u.path());
        u.set_path(&p);
    }
    Ok(u)
}

fn sub(base: &Url, p: &str) -> Url {
    base.join(p).expect("joinable")
}

/// Parse and self-verify a root document (signature threshold over itself).
pub fn parse_root(bytes: &[u8]) -> Result<Signed<Root>> {
    let root: Signed<Root> = serde_json::from_slice(bytes)
        .map_err(|e| Error::preflight("INVALID_ROOT", format!("root metadata is not valid TUF root JSON: {e}")))?;
    root.signed
        .verify_role(&root)
        .map_err(|e| Error::preflight("INVALID_ROOT", format!("root metadata is not signed by its own root keys: {e}")))?;
    Ok(root)
}

/// Fetch `metadata/1.root.json` for first-use display. Untrusted until the user approves it.
pub async fn fetch_initial_root(base: &Url, dev: bool) -> Result<Vec<u8>> {
    let t = W3bsTransport::new(dev);
    let url = sub(base, "metadata/1.root.json");
    let stream = t
        .fetch(url.clone())
        .await
        .map_err(|e| Error::package("REPOSITORY_UNAVAILABLE", format!("cannot fetch {url}: {e}")).retryable())?;
    let mut buf = Vec::new();
    let mut s = stream;
    while let Some(chunk) = s.try_next().await.map_err(|e| Error::package("REPOSITORY_UNAVAILABLE", format!("{url}: {e}")).retryable())? {
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > 1024 * 1024 {
            return Err(Error::preflight("INVALID_ROOT", "root metadata exceeds 1 MiB"));
        }
    }
    Ok(buf)
}

pub struct LoadedRepo {
    pub alias: String,
    pub repo_key: String,
    pub fingerprint: String,
    pub repo: Repository,
}

/// Write the user-approved initial root into the repository's trust directory.
pub fn store_initial_root(layout: &Layout, key: &str, bytes: &[u8]) -> Result<()> {
    let dir = layout.trust_dir(key);
    crate::layout::create_private_dir(&dir)?;
    let p = dir.join("initial-root.json");
    if p.exists() {
        // Same key means same bytes (the key is their digest); keep the original.
        return Ok(());
    }
    crate::fsutil::write_durable(&p, bytes)
}

/// Load and verify a configured repository's metadata through TUF.
pub async fn load(layout: &Layout, alias: &str, base_url: &str, key: &str) -> Result<LoadedRepo> {
    let dir = layout.trust_dir(key);
    let initial = std::fs::read(dir.join("initial-root.json"))
        .map_err(|e| Error::preflight("TRUST_STATE", format!("trust root for repository `{alias}` is missing: {e}; run `w3bs repo remove {alias}` and add it again")))?;
    // The datastore holds the newest root reached through verified rotation; prefer it.
    let anchor = std::fs::read(dir.join("root.json")).unwrap_or_else(|_| initial.clone());
    let base = normalize_base(base_url, layout.dev_transport)?;
    let repo = RepositoryLoader::new(&anchor, sub(&base, "metadata/"), sub(&base, "targets/"))
        .transport(W3bsTransport::new(layout.dev_transport))
        .datastore(dir.clone())
        .load()
        .await
        .map_err(|e| classify_tough(alias, e))?;
    Ok(LoadedRepo { alias: alias.to_owned(), repo_key: key.to_owned(), fingerprint: fingerprint(&initial), repo })
}

fn classify_tough(alias: &str, e: tough::error::Error) -> Error {
    use tough::error::Error as T;
    let msg = format!("repository `{alias}`: {e}");
    match e {
        T::Transport { .. } => Error::package("REPOSITORY_UNAVAILABLE", msg).retryable(),
        T::ExpiredMetadata { .. } => Error::package("METADATA_EXPIRED", format!("{msg} (check the system clock if this looks wrong)")),
        T::OlderMetadata { .. } => Error::package("METADATA_ROLLBACK", msg),
        T::VerifyMetadata { .. } | T::VerifyTrustedMetadata { .. } | T::VerifyRoleMetadata { .. } => Error::package("METADATA_SIGNATURE", msg),
        T::HashMismatch { .. } => Error::package("DIGEST_MISMATCH", msg),
        T::MaxSizeExceeded { .. } | T::MaxUpdatesExceeded { .. } => Error::package("SIZE_LIMIT", msg),
        _ => Error::package("REPOSITORY_INVALID", msg),
    }
}

impl LoadedRepo {
    /// Read a target, verified by TUF (length and hashes from signed targets metadata).
    pub async fn read_target(&self, path: &str, max: u64) -> Result<Vec<u8>> {
        let name = TargetName::new(path).map_err(|e| Error::package("INVALID_TARGET", format!("invalid target name `{path}`: {e}")))?;
        let target = self
            .repo
            .targets()
            .signed
            .find_target(&name, false)
            .map_err(|_| Error::package("TARGET_NOT_FOUND", format!("`{path}` is not a signed target of repository `{}`", self.alias)))?;
        if target.length > max {
            return Err(Error::package("SIZE_LIMIT", format!("target `{path}` is {} bytes, above the {max} byte limit", target.length)));
        }
        let stream = self
            .repo
            .read_target(&name)
            .await
            .map_err(|e| classify_tough(&self.alias, e))?
            .ok_or_else(|| Error::package("TARGET_NOT_FOUND", format!("`{path}` is not a signed target of `{}`", self.alias)))?;
        stream.into_vec().await.map_err(|e| classify_tough(&self.alias, e))
    }

    /// Signed length + sha256 for a target, from targets metadata.
    pub fn target_info(&self, path: &str) -> Option<(u64, String)> {
        let name = TargetName::new(path).ok()?;
        let t = self.repo.targets().signed.find_target(&name, false).ok()?;
        Some((t.length, hex::encode(&t.hashes.sha256)))
    }

    pub async fn package_index(&self) -> Result<PackageIndex> {
        let b = self.read_target(PACKAGE_INDEX_TARGET, MAX_DOCUMENT).await?;
        PackageIndex::parse(&b).map_err(|e| Error::package("INVALID_INDEX", format!("repository `{}`: {e}", self.alias)))
    }

    /// The repository's curl catalog; an absent target means an empty catalog.
    pub async fn catalog(&self) -> Result<CatalogIndex> {
        if self.target_info(CATALOG_INDEX_TARGET).is_none() {
            return Ok(CatalogIndex { schema_version: w3bs_schema::manifest::SCHEMA_VERSION.into(), entries: vec![] });
        }
        let b = self.read_target(CATALOG_INDEX_TARGET, MAX_DOCUMENT).await?;
        CatalogIndex::parse(&b).map_err(|e| Error::package("INVALID_CATALOG", format!("repository `{}`: {e}", self.alias)))
    }

    /// The repository's package index; an absent target means no native packages.
    pub async fn package_index_or_empty(&self) -> Result<PackageIndex> {
        if self.target_info(PACKAGE_INDEX_TARGET).is_none() {
            return Ok(PackageIndex { schema_version: w3bs_schema::manifest::SCHEMA_VERSION.into(), packages: vec![] });
        }
        self.package_index().await
    }

    pub async fn channel(&self, path: &str) -> Result<ChannelIndex> {
        let b = self.read_target(path, MAX_DOCUMENT).await?;
        ChannelIndex::parse(&b).map_err(|e| Error::package("INVALID_CHANNEL", format!("repository `{}`: {e}", self.alias)))
    }

    pub async fn manifest(&self, path: &str) -> Result<Manifest> {
        let b = self.read_target(path, MAX_DOCUMENT).await?;
        Manifest::parse(&b).map_err(|e| Error::package("INVALID_MANIFEST", format!("repository `{}`: {e}", self.alias)))
    }

    pub fn root_version(&self) -> u64 {
        self.repo.root().signed.version.get()
    }
}
