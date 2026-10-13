//! W3BS resources: `w3bs://type/namespace/name@version` manifests in a W3BS registry
//! (https://w3bs.org by default). A port of the reference Node client: every manifest the
//! registry returns is re-verified locally (RFC 8785 canonical JSON + Ed25519 against a
//! pinned trust store), and a disagreement with the registry is an error, not a warning.

use crate::error::{Error, Result};
use aws_lc_rs::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

pub const TYPES: &[&str] = &["prompt", "behavior", "agent", "tool", "mcp", "schema", "eval", "identity", "service", "device", "stream", "swarm"];

const DEFAULT_TRUST: &str = include_str!("../../../../fixtures/trust.json");
const SPKI_ED25519_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
const PKCS8_V1_ED25519_PREFIX: [u8; 16] = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uri {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub version: Option<String>,
}

impl Uri {
    pub fn base(&self) -> String {
        format!("w3bs://{}/{}/{}", self.kind, self.namespace, self.name)
    }
}

fn segment_ok(s: &str) -> bool {
    !s.is_empty() && s.split('-').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

fn int_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

pub fn parse_uri(value: &str) -> Result<Uri> {
    let bad = || {
        Error::usage(
            "INVALID_URI",
            "expected w3bs://type/namespace/name with an optional @major or @major.minor.patch (lowercase ASCII; no queries, fragments or escapes)",
        )
    };
    if value.len() > 512 {
        return Err(bad());
    }
    let rest = value.strip_prefix("w3bs://").ok_or_else(bad)?;
    let (path, version) = match rest.split_once('@') {
        Some((p, v)) => (p, Some(v)),
        None => (rest, None),
    };
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 3 || !TYPES.contains(&parts[0]) || !segment_ok(parts[1]) || !segment_ok(parts[2]) {
        return Err(bad());
    }
    if let Some(v) = version {
        let n: Vec<&str> = v.split('.').collect();
        if !(n.len() == 1 || n.len() == 3) || !n.iter().all(|x| int_ok(x)) {
            return Err(bad());
        }
    }
    Ok(Uri { kind: parts[0].into(), namespace: parts[1].into(), name: parts[2].into(), version: version.map(Into::into) })
}

/// RFC 8785 canonical JSON of the manifest without its `proof`.
pub fn signed_bytes(manifest: &Value) -> Result<Vec<u8>> {
    let mut payload = manifest.clone();
    if let Some(o) = payload.as_object_mut() {
        o.remove("proof");
    }
    serde_jcs::to_vec(&payload).map_err(|e| Error::usage("INVALID_MANIFEST", format!("cannot canonicalize manifest: {e}")))
}

pub fn digest(manifest: &Value) -> Result<String> {
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(signed_bytes(manifest)?))))
}

fn pem_body(pem: &str) -> Option<Vec<u8>> {
    let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    STANDARD.decode(b64.trim()).ok()
}

fn ed25519_public_from_spki_pem(pem: &str) -> Option<[u8; 32]> {
    let der = pem_body(pem)?;
    if der.len() != 44 || der[..12] != SPKI_ED25519_PREFIX {
        return None;
    }
    der[12..].try_into().ok()
}

pub fn trust_store() -> Result<Value> {
    let raw = match std::env::var("W3BS_TRUST_FILE") {
        Ok(p) => std::fs::read_to_string(&p).map_err(|e| Error::usage("TRUST_FILE", format!("cannot read {p}: {e}")))?,
        Err(_) => DEFAULT_TRUST.to_owned(),
    };
    serde_json::from_str(&raw).map_err(|e| Error::usage("TRUST_FILE", format!("invalid trust store: {e}")))
}

/// Same result shape and rules as the reference `verifyManifest`.
pub fn verify_manifest(manifest: &Value, trust: &Value, revoked: bool) -> Result<Value> {
    let proof = &manifest["proof"];
    let key_id = proof["keyId"].as_str().unwrap_or_default();
    let key = trust["keys"].as_array().and_then(|ks| ks.iter().find(|k| k["id"] == key_id));
    let d = digest(manifest)?;
    let mut signature_valid = false;
    if let Some(k) = key
        && let Some(public) = k["publicKeyPem"].as_str().and_then(ed25519_public_from_spki_pem)
        && proof["digest"].as_str() == Some(d.as_str())
        && let Some(sig) = proof["signature"].as_str().and_then(|s| URL_SAFE_NO_PAD.decode(s).ok())
    {
        signature_valid = UnparsedPublicKey::new(&ED25519, public).verify(&signed_bytes(manifest)?, &sig).is_ok();
    }
    let publisher_trusted = key.is_some_and(|k| k["namespace"] == manifest["publisher"]["namespace"] && k["revoked"] != true);
    let key_revoked = key.is_some_and(|k| k["revoked"] == true);
    Ok(json!({
        "valid": signature_valid && publisher_trusted && !revoked,
        "signatureValid": signature_valid,
        "publisherTrusted": publisher_trusted,
        "revoked": revoked || key_revoked,
        "digest": d,
        "keyId": key_id,
        "authority": "DATA",
    }))
}

/// Sign a manifest with an Ed25519 PKCS#8 PEM private key (as written by `keygen`).
pub fn sign_manifest(manifest: &Value, private_pem: &str) -> Result<Value> {
    let der = pem_body(private_pem).ok_or_else(|| Error::usage("INVALID_KEY", "private key is not PEM"))?;
    let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|_| Error::usage("INVALID_KEY", "private key is not an Ed25519 PKCS#8 key"))?;
    let key_id = manifest["publisher"]["keyId"].as_str().ok_or_else(|| Error::usage("INVALID_MANIFEST", "manifest has no publisher.keyId"))?;
    parse_uri(manifest["id"].as_str().unwrap_or_default())?;
    let mut out = manifest.clone();
    let obj = out.as_object_mut().ok_or_else(|| Error::usage("INVALID_MANIFEST", "manifest must be a JSON object"))?;
    obj.remove("proof");
    let bytes = signed_bytes(&out)?;
    let sig = pair.sign(&bytes);
    out["proof"] = json!({
        "type": "Ed25519",
        "keyId": key_id,
        "digest": digest(&out)?,
        "signature": URL_SAFE_NO_PAD.encode(sig.as_ref()),
    });
    Ok(out)
}

fn pem(label: &str, der: &[u8]) -> String {
    let b64 = STANDARD.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// Generate a publisher key: `private.pem` (PKCS#8, 0600) and `public.json` (trust entry).
pub fn keygen(namespace: &str, dir: &Path) -> Result<Value> {
    parse_uri(&format!("w3bs://identity/{namespace}/publisher"))?;
    std::fs::create_dir_all(dir).map_err(|e| Error::usage("OUT_DIR", format!("{}: {e}", dir.display())))?;
    let mut seed = [0u8; 32];
    aws_lc_rs::rand::fill(&mut seed).map_err(|_| Error::preflight("RNG", "no system randomness"))?;
    let der = [PKCS8_V1_ED25519_PREFIX.as_slice(), &seed].concat();
    let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|_| Error::preflight("KEYGEN", "key generation failed"))?;
    let spki = [SPKI_ED25519_PREFIX.as_slice(), pair.public_key().as_ref()].concat();
    let private = dir.join("private.pem");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&private).map_err(|e| Error::usage("OUT_EXISTS", format!("{}: {e}", private.display())))?;
        f.write_all(pem("PRIVATE KEY", &der).as_bytes())?;
    }
    let key = json!({ "id": format!("{namespace}-1"), "namespace": namespace, "publicKeyPem": pem("PUBLIC KEY", &spki), "revoked": false });
    write_new(&dir.join("public.json"), &key)?;
    Ok(json!({
        "directory": dir,
        "publicKey": key,
        "next": "Ask the registry operator to bind this public key to your namespace. Never send the private key.",
    }))
}

pub fn write_new(path: &Path, v: &Value) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| Error::usage("OUT_EXISTS", format!("{}: {e}", path.display())))?;
    f.write_all(format!("{}\n", serde_json::to_string_pretty(v).unwrap()).as_bytes())?;
    Ok(())
}

pub struct Registry {
    origin: url::Url,
    token: Option<String>,
    client: reqwest::Client,
    trust: Value,
}

impl Registry {
    /// `W3BS_API` (default https://w3bs.org): an origin, HTTPS except loopback.
    pub fn from_env(api: Option<&str>) -> Result<Self> {
        let raw = api.map(str::to_owned).or_else(|| std::env::var("W3BS_API").ok()).unwrap_or_else(|| "https://w3bs.org".into());
        let origin = url::Url::parse(&raw).map_err(|e| Error::usage("INVALID_API", format!("{raw}: {e}")))?;
        let loopback = matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !origin.username().is_empty() || origin.password().is_some() || origin.query().is_some() || origin.fragment().is_some() || origin.path() != "/" {
            return Err(Error::usage("INVALID_API", "the registry must be an origin without credentials, path, query or fragment"));
        }
        if origin.scheme() != "https" && !(origin.scheme() == "http" && loopback) {
            return Err(Error::usage("HTTPS_REQUIRED", "use HTTPS except for loopback development"));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("w3bs/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Ok(Self { origin, token: std::env::var("W3BS_PUBLISH_TOKEN").ok().filter(|t| !t.is_empty()), client, trust: trust_store()? })
    }

    /// Call `/api/<operation>` and verify any manifest in the answer locally.
    pub async fn call(&self, operation: &str, args: Value) -> Result<Value> {
        let url = self.origin.join(&format!("api/{operation}")).unwrap();
        let mut req = self.client.post(url).json(&args);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.map_err(|e| Error::package("REGISTRY_UNAVAILABLE", format!("{}: {e}", self.origin)).retryable())?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| Error::package("REGISTRY_UNAVAILABLE", e.to_string()).retryable())?;
        if bytes.len() > 2_000_000 {
            return Err(Error::package("RESPONSE_TOO_LARGE", "registry response exceeded 2 MB"));
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| Error::package("REGISTRY_INVALID", format!("registry answered HTTP {status} without JSON")))?;
        if !status.is_success() {
            let code = v["error"]["code"].as_str().unwrap_or("REQUEST_FAILED");
            let msg = v["error"]["message"].as_str().unwrap_or("request failed").to_owned();
            let code: &'static str = Box::leak(code.to_owned().into_boxed_str());
            return Err(if status.as_u16() == 400 || status.as_u16() == 404 { Error::usage(code, msg) } else { Error::package(code, msg) });
        }
        if v.get("manifest").is_some() {
            let local = verify_manifest(&v["manifest"], &self.trust, v["verification"]["revoked"] == true)?;
            if v["canonicalUri"] != v["manifest"]["id"] || local != v["verification"] {
                return Err(Error::preflight("SURFACE_MISMATCH", "local manifest verification disagrees with the registry"));
            }
            let requested = parse_uri(args["uri"].as_str().or_else(|| args["manifest"]["id"].as_str()).unwrap_or_default())?;
            let actual = parse_uri(v["canonicalUri"].as_str().unwrap_or_default())?;
            let version_ok = match &requested.version {
                None => true,
                Some(r) if r.contains('.') => actual.version.as_deref() == Some(r.as_str()),
                Some(r) => actual.version.as_deref().and_then(|a| a.split('.').next()) == Some(r.as_str()),
            };
            if requested.base() != actual.base() || !version_ok {
                return Err(Error::preflight("SURFACE_MISMATCH", "the registry returned a different resource than requested"));
            }
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_grammar() {
        assert!(parse_uri("w3bs://prompt/w3bs/research@1").is_ok());
        assert!(parse_uri("w3bs://prompt/w3bs/research@1.0.0").is_ok());
        assert!(parse_uri("w3bs://prompt/w3bs/research").is_ok());
        for bad in ["w3bs://nope/w3bs/x", "w3bs://prompt/W3bs/x", "w3bs://prompt/w3bs/x@1.0", "w3bs://prompt/w3bs/x@01", "https://x", "w3bs://prompt/w3bs/x?y"] {
            assert!(parse_uri(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reference_fixture_verifies_like_node() {
        let m: Value = serde_json::from_str(include_str!("../../../../fixtures/resources/research.json")).unwrap();
        let v = verify_manifest(&m, &trust_store().unwrap(), false).unwrap();
        assert_eq!(v["valid"], true, "{v}");
        assert_eq!(v["signatureValid"], true);
        let mut tampered = m.clone();
        tampered["description"] = "changed".into();
        assert_eq!(verify_manifest(&tampered, &trust_store().unwrap(), false).unwrap()["valid"], false);
    }

    #[test]
    fn keygen_sign_verify_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let out = keygen("example", d.path()).unwrap();
        let mut m: Value = serde_json::from_str(include_str!("../../../../fixtures/resources/research.json")).unwrap();
        m["publisher"]["namespace"] = "example".into();
        m["publisher"]["keyId"] = "example-1".into();
        m["id"] = "w3bs://prompt/example/research@1.0.0".into();
        let signed = sign_manifest(&m, &std::fs::read_to_string(d.path().join("private.pem")).unwrap()).unwrap();
        let trust = json!({ "keys": [out["publicKey"]] });
        assert_eq!(verify_manifest(&signed, &trust, false).unwrap()["valid"], true);
    }
}
