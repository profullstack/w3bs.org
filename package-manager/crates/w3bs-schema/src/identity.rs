//! Publisher-qualified package identities and command names.
//!
//! An identity is `<publisher-host>/<name>`, ASCII only. The host is a namespace label,
//! never proof of signing authority: authority comes from the repository trust root an
//! installation is bound to.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

/// Command names a package may never claim, because the manager owns them.
pub const RESERVED_COMMANDS: &[&str] = &["w3bs", "w3bs-publish"];

/// The manager's own package identity.
pub const MANAGER_PACKAGE_ID: &str = "w3bs.org/w3bs";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("package identity `{0}` must look like `publisher.example/name`")]
    Shape(String),
    #[error("publisher `{0}` is not a valid lowercase hostname")]
    Publisher(String),
    #[error("package name `{0}` must be 1-64 chars of a-z, 0-9, `-`, `_`, starting with a letter or digit")]
    Name(String),
    #[error("command name `{0}` must be 1-64 chars of a-z, A-Z, 0-9, `-`, `_`, `.`, and not start with `.` or `-`")]
    Command(String),
    #[error("command name `{0}` is reserved for W3BS Package Manager")]
    ReservedCommand(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageId {
    publisher: String,
    name: String,
}

impl PackageId {
    pub fn new(publisher: &str, name: &str) -> Result<Self, IdentityError> {
        validate_publisher(publisher)?;
        validate_name(name)?;
        Ok(Self { publisher: publisher.to_owned(), name: name.to_owned() })
    }

    pub fn publisher(&self) -> &str {
        &self.publisher
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Filesystem-safe opaque key. Never concatenate untrusted names into paths directly:
    /// the readable prefix is already validated, and the digest keeps keys unique.
    pub fn storage_key(&self) -> String {
        if self.is_manager() {
            return "w3bs-manager".to_owned();
        }
        let digest = Sha256::digest(self.to_string().as_bytes());
        format!("{}-{}", self.name, &hex::encode(digest)[..16])
    }

    pub fn is_manager(&self) -> bool {
        self.to_string() == MANAGER_PACKAGE_ID
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.publisher, self.name)
    }
}

impl FromStr for PackageId {
    type Err = IdentityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (publisher, name) = s.split_once('/').ok_or_else(|| IdentityError::Shape(s.to_owned()))?;
        if name.contains('/') {
            return Err(IdentityError::Shape(s.to_owned()));
        }
        Self::new(publisher, name)
    }
}

impl TryFrom<String> for PackageId {
    type Error = IdentityError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<PackageId> for String {
    fn from(id: PackageId) -> String {
        id.to_string()
    }
}

/// What a user typed: a qualified identity or a bare short name to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageRef {
    Qualified(PackageId),
    Short(String),
}

impl FromStr for PackageRef {
    type Err = IdentityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains('/') {
            Ok(Self::Qualified(s.parse()?))
        } else {
            validate_name(s)?;
            Ok(Self::Short(s.to_owned()))
        }
    }
}

impl fmt::Display for PackageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qualified(id) => id.fmt(f),
            Self::Short(s) => f.write_str(s),
        }
    }
}

impl PackageRef {
    pub fn matches(&self, id: &PackageId) -> bool {
        match self {
            Self::Qualified(q) => q == id,
            Self::Short(s) => id.name() == s,
        }
    }
}

fn validate_publisher(host: &str) -> Result<(), IdentityError> {
    let bad = || IdentityError::Publisher(host.to_owned());
    if host.is_empty() || host.len() > 253 || !host.contains('.') {
        return Err(bad());
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(bad());
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), IdentityError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok { Ok(()) } else { Err(IdentityError::Name(name.to_owned())) }
}

/// Validate a command name that will become a launch link in the user's bin directory.
pub fn validate_command(name: &str, allow_reserved: bool) -> Result<(), IdentityError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if !ok {
        return Err(IdentityError::Command(name.to_owned()));
    }
    if !allow_reserved && RESERVED_COMMANDS.contains(&name) {
        return Err(IdentityError::ReservedCommand(name.to_owned()));
    }
    Ok(())
}

/// Validate a relative path inside a package (manifest command target, archive entry).
/// Rejects absolute paths, `.`/`..` components, empty components, backslashes and controls.
pub fn validate_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(|c| c.is_control())
        && path.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qualified_ids() {
        let id: PackageId = "tools.example/atlas".parse().unwrap();
        assert_eq!(id.publisher(), "tools.example");
        assert_eq!(id.name(), "atlas");
        assert_eq!(id.to_string(), "tools.example/atlas");
    }

    #[test]
    fn rejects_bad_ids() {
        for bad in [
            "atlas",
            "/atlas",
            "tools.example/",
            "tools.example/a/b",
            "Tools.example/atlas",
            "localhost/atlas",
            "tools.example/Atlas",
            "tools.example/../x",
            "tools.example/at las",
            "tools..example/atlas",
            "tools.example/atlas\u{0}",
            "t\u{043e}ols.example/atlas", // Cyrillic o
        ] {
            assert!(bad.parse::<PackageId>().is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn storage_keys_are_safe_and_distinct() {
        let a: PackageId = "tools.example/atlas".parse().unwrap();
        let b: PackageId = "labs.example/atlas".parse().unwrap();
        assert_ne!(a.storage_key(), b.storage_key());
        assert!(a.storage_key().starts_with("atlas-"));
        let m: PackageId = MANAGER_PACKAGE_ID.parse().unwrap();
        assert_eq!(m.storage_key(), "w3bs-manager");
    }

    #[test]
    fn command_rules() {
        assert!(validate_command("atlas", false).is_ok());
        assert!(validate_command("w3bs", false).is_err());
        assert!(validate_command("w3bs", true).is_ok());
        assert!(validate_command("../x", false).is_err());
        assert!(validate_command(".hidden", false).is_err());
        assert!(validate_command("a/b", false).is_err());
    }

    #[test]
    fn relative_paths() {
        assert!(validate_relative_path("bin/atlas"));
        assert!(!validate_relative_path("/bin/atlas"));
        assert!(!validate_relative_path("bin/../../etc"));
        assert!(!validate_relative_path("./bin"));
        assert!(!validate_relative_path("bin//x"));
        assert!(!validate_relative_path("bin\\x"));
    }
}
