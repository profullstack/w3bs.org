//! Release manifests, repository package indexes and channel indexes.
//!
//! Every one of these documents is a TUF target authenticated by the repository's signed
//! metadata. Parsing one here proves nothing about where it came from.

use crate::identity::{PackageId, validate_command, validate_relative_path};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The major schema version this implementation understands. Unknown majors fail closed.
pub const SCHEMA_MAJOR: u64 = 1;
pub const SCHEMA_VERSION: &str = "1.0";

/// Capabilities this implementation can honour. P0 honours none, so any declared required
/// capability fails closed.
pub const SUPPORTED_CAPABILITIES: &[&str] = &[];

pub const PACKAGE_INDEX_TARGET: &str = "packages/index.json";

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("unsupported schema_version `{0}` (this client understands {SCHEMA_MAJOR}.x)")]
    UnsupportedSchema(String),
    #[error("invalid document: {0}")]
    Invalid(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub fn check_schema_version(v: &str) -> Result<(), SchemaError> {
    let major = v.split('.').next().and_then(|m| m.parse::<u64>().ok());
    if major == Some(SCHEMA_MAJOR) { Ok(()) } else { Err(SchemaError::UnsupportedSchema(v.to_owned())) }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactFormat {
    #[serde(rename = "tar.gz")]
    TarGz,
    /// A single executable; requires exactly one command mapping.
    #[serde(rename = "binary")]
    Binary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: String,
    pub package: PackageInfo,
    pub release: ReleaseInfo,
    #[serde(default)]
    pub requirements: Requirements,
    pub artifacts: Vec<Artifact>,
    /// Optional extensions live in an explicit namespace and are never executable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageInfo {
    pub id: PackageId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: Version,
    pub channel: String,
    pub published_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Requirements {
    /// A semver requirement on the manager, e.g. `>=0.1.0, <0.2.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    pub platform: String,
    pub target_path: String,
    pub format: ArtifactFormat,
    pub length: u64,
    pub sha256: String,
    pub unpacked_size_limit: u64,
    pub commands: BTreeMap<String, String>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, SchemaError> {
        let m: Manifest = serde_json::from_slice(bytes)?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        check_schema_version(&self.schema_version)?;
        let invalid = |s: String| Err(SchemaError::Invalid(s));
        if self.package.name != self.package.id.name() {
            return invalid(format!("package.name `{}` does not match id `{}`", self.package.name, self.package.id));
        }
        validate_channel(&self.release.channel).map_err(SchemaError::Invalid)?;
        if self.release.channel == "stable" && !self.release.version.pre.is_empty() {
            return invalid("a prerelease version cannot be published to the stable channel".into());
        }
        if let Some(req) = &self.requirements.manager {
            semver::VersionReq::parse(req).map_err(|e| SchemaError::Invalid(format!("requirements.manager: {e}")))?;
        }
        for cap in &self.requirements.capabilities {
            if !SUPPORTED_CAPABILITIES.contains(&cap.as_str()) {
                return invalid(format!("required capability `{cap}` is not supported by this client"));
            }
        }
        if self.artifacts.is_empty() {
            return invalid("no artifacts".into());
        }
        let mut platforms = std::collections::BTreeSet::new();
        for a in &self.artifacts {
            if !platforms.insert(&a.platform) {
                return invalid(format!("duplicate artifact for platform `{}`", a.platform));
            }
            if !validate_relative_path(&a.target_path) {
                return invalid(format!("unsafe target_path `{}`", a.target_path));
            }
            if a.sha256.len() != 64 || !a.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                return invalid(format!("artifact sha256 for `{}` must be 64 lowercase hex chars", a.platform));
            }
            if a.length == 0 || a.unpacked_size_limit == 0 {
                return invalid("artifact length and unpacked_size_limit must be positive".into());
            }
            if a.commands.is_empty() {
                return invalid(format!("artifact `{}` declares no commands", a.platform));
            }
            if a.format == ArtifactFormat::Binary && a.commands.len() != 1 {
                return invalid("a binary artifact maps exactly one command".into());
            }
            for (cmd, path) in &a.commands {
                validate_command(cmd, self.package.id.is_manager()).map_err(|e| SchemaError::Invalid(e.to_string()))?;
                if !validate_relative_path(path) {
                    return invalid(format!("unsafe command path `{path}`"));
                }
            }
        }
        Ok(())
    }

    pub fn artifact_for(&self, platform: &str) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.platform == platform)
    }
}

pub fn validate_channel(c: &str) -> Result<(), String> {
    let ok = !c.is_empty() && c.len() <= 32 && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok { Ok(()) } else { Err(format!("invalid channel name `{c}`")) }
}

/// `packages/index.json`: what a repository offers. Candidates, not ownership.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageIndex {
    pub schema_version: String,
    pub packages: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexEntry {
    pub id: PackageId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// channel name -> channel-index target path
    pub channels: BTreeMap<String, String>,
}

impl PackageIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, SchemaError> {
        let i: PackageIndex = serde_json::from_slice(bytes)?;
        check_schema_version(&i.schema_version)?;
        for e in &i.packages {
            for (ch, path) in &e.channels {
                validate_channel(ch).map_err(SchemaError::Invalid)?;
                if !validate_relative_path(path) {
                    return Err(SchemaError::Invalid(format!("unsafe channel path `{path}`")));
                }
            }
        }
        Ok(i)
    }
}

/// `channels/<name>/<channel>.json`: releases on a channel plus explicit revocations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelIndex {
    pub schema_version: String,
    pub package_id: PackageId,
    pub channel: String,
    pub releases: Vec<ChannelRelease>,
    /// Omission of a release is not revocation; only entries here are.
    #[serde(default)]
    pub revoked: Vec<Revocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelRelease {
    pub version: Version,
    pub manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Revocation {
    pub sha256: String,
    pub reason: String,
}

impl ChannelIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, SchemaError> {
        let c: ChannelIndex = serde_json::from_slice(bytes)?;
        check_schema_version(&c.schema_version)?;
        validate_channel(&c.channel).map_err(SchemaError::Invalid)?;
        for r in &c.releases {
            if !validate_relative_path(&r.manifest) {
                return Err(SchemaError::Invalid(format!("unsafe manifest path `{}`", r.manifest)));
            }
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        serde_json::from_value(serde_json::json!({
          "schema_version": "1.0",
          "package": {"id": "tools.example/atlas", "name": "atlas", "description": "x", "license": "MIT"},
          "release": {"version": "1.2.3", "channel": "stable", "published_at": "2026-10-12T12:00:00Z"},
          "requirements": {"manager": ">=0.1.0, <0.2.0", "capabilities": []},
          "artifacts": [{
            "platform": "aarch64-apple-darwin",
            "target_path": "artifacts/atlas/1.2.3/atlas-aarch64-apple-darwin.tar.gz",
            "format": "tar.gz", "length": 5, "sha256": "0".repeat(64),
            "unpacked_size_limit": 100, "commands": {"atlas": "bin/atlas"}
          }]
        }))
        .unwrap()
    }

    #[test]
    fn prd_example_manifest_validates() {
        sample().validate().unwrap();
    }

    #[test]
    fn unknown_major_and_capability_fail_closed() {
        let mut m = sample();
        m.schema_version = "2.0".into();
        assert!(m.validate().is_err());
        let mut m = sample();
        m.requirements.capabilities = vec!["post-install-hook".into()];
        assert!(m.validate().is_err());
    }

    #[test]
    fn unsafe_paths_and_reserved_commands_rejected() {
        let mut m = sample();
        m.artifacts[0].commands.insert("atlas".into(), "../../etc/passwd".into());
        assert!(m.validate().is_err());
        let mut m = sample();
        m.artifacts[0].commands = [("w3bs".to_string(), "bin/w3bs".to_string())].into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn prerelease_not_on_stable() {
        let mut m = sample();
        m.release.version = "1.3.0-beta.1".parse().unwrap();
        assert!(m.validate().is_err());
        m.release.channel = "beta".into();
        m.validate().unwrap();
    }
}

pub const CATALOG_INDEX_TARGET: &str = "catalog/index.json";

/// `catalog/index.json`: tools installed by their publisher's own `curl | sh` installer.
///
/// A catalog entry is listed by a signed repository, so the *entry* (identity, installer
/// URL) is authenticated; the installer script itself is the publisher's and is not.
/// W3BS runs that script and nothing else: it never calls npm, pip or brew directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogIndex {
    pub schema_version: String,
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: PackageId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<String>,
    pub install: CurlInstall,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub agentic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CurlInstall {
    /// HTTPS URL of the installer script.
    pub url: String,
    /// Interpreter the publisher documents: `sh` or `bash`.
    #[serde(default = "default_shell")]
    pub shell: String,
}

fn default_shell() -> String {
    "sh".into()
}

impl CurlInstall {
    /// The exact command a human would paste.
    pub fn command(&self) -> String {
        format!("curl -fsSL {} | {}", self.url, self.shell)
    }
}

impl CatalogIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, SchemaError> {
        let c: CatalogIndex = serde_json::from_slice(bytes)?;
        check_schema_version(&c.schema_version)?;
        for e in &c.entries {
            e.validate()?;
        }
        Ok(c)
    }
}

impl CatalogEntry {
    pub fn validate(&self) -> Result<(), SchemaError> {
        if self.name != self.id.name() {
            return Err(SchemaError::Invalid(format!("catalog name `{}` does not match id `{}`", self.name, self.id)));
        }
        if !self.install.url.starts_with("https://") || self.install.url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(SchemaError::Invalid(format!("installer for `{}` must be a plain https:// URL", self.id)));
        }
        if !matches!(self.install.shell.as_str(), "sh" | "bash") {
            return Err(SchemaError::Invalid(format!("installer shell for `{}` must be sh or bash", self.id)));
        }
        for c in &self.commands {
            validate_command(c, false).map_err(|e| SchemaError::Invalid(e.to_string()))?;
        }
        Ok(())
    }
}
