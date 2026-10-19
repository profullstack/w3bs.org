//! OpenSSH signatures (SSHSIG, `ssh-keygen -Y sign`) for W3BS-SIGN-1 publisher signing.
//!
//! Format: OpenSSH PROTOCOL.sshsig. Accepted keys are the four W3BS-SIGN-1 types
//! (`ssh-ed25519`, `sk-ssh-ed25519@openssh.com`, `ecdsa-sha2-nistp256`,
//! `sk-ecdsa-sha2-nistp256@openssh.com`); RSA and DSA are refused. Verification uses
//! aws-lc-rs, the same crypto as the rest of the engine.
//!
//! Hardware (`sk-`) keys sign `sha256(application) || flags || counter || sha256(data)`
//! rather than the data itself; [`verify`] rebuilds that and reports the flags and counter.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::{Digest, Sha256, Sha512};

/// SSHSIG namespace for release statements.
pub const NS_RELEASE: &str = "w3bs-release@w3bs.org";
/// SSHSIG namespace for key registration challenges.
pub const NS_KEY: &str = "w3bs-key@w3bs.org";

const MAGIC: &[u8; 6] = b"SSHSIG";
const BEGIN: &str = "-----BEGIN SSH SIGNATURE-----";
const END: &str = "-----END SSH SIGNATURE-----";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SigError {
    #[error("malformed SSH signature: {0}")]
    Malformed(String),
    #[error("unsupported key type {0} (W3BS-SIGN-1 accepts ssh-ed25519, sk-ssh-ed25519@openssh.com, ecdsa-sha2-nistp256 and sk-ecdsa-sha2-nistp256@openssh.com; RSA and DSA are refused)")]
    UnsupportedKey(String),
    #[error("unsupported hash algorithm {0} (sha256 or sha512)")]
    UnsupportedHash(String),
    #[error("signature namespace is {found:?}, expected {expected:?}")]
    WrongNamespace { expected: String, found: String },
    #[error("signature was made by {found}, expected {expected}")]
    WrongKey { expected: String, found: String },
    #[error("signature does not verify")]
    BadSignature,
}

pub type Result<T> = std::result::Result<T, SigError>;

fn malformed(m: impl Into<String>) -> SigError {
    SigError::Malformed(m.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Ed25519,
    SkEd25519,
    EcdsaP256,
    SkEcdsaP256,
}

impl KeyType {
    pub fn name(self) -> &'static str {
        match self {
            KeyType::Ed25519 => "ssh-ed25519",
            KeyType::SkEd25519 => "sk-ssh-ed25519@openssh.com",
            KeyType::EcdsaP256 => "ecdsa-sha2-nistp256",
            KeyType::SkEcdsaP256 => "sk-ecdsa-sha2-nistp256@openssh.com",
        }
    }

    fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "ssh-ed25519" => KeyType::Ed25519,
            "sk-ssh-ed25519@openssh.com" => KeyType::SkEd25519,
            "ecdsa-sha2-nistp256" => KeyType::EcdsaP256,
            "sk-ecdsa-sha2-nistp256@openssh.com" => KeyType::SkEcdsaP256,
            other => return Err(SigError::UnsupportedKey(other.to_owned())),
        })
    }

    pub fn is_sk(self) -> bool {
        matches!(self, KeyType::SkEd25519 | KeyType::SkEcdsaP256)
    }
}

/// An SSH public key of an accepted type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    pub key_type: KeyType,
    /// The SSH wire-format key blob (what fingerprints hash).
    pub blob: Vec<u8>,
    /// Ed25519: 32 bytes. ECDSA: the uncompressed SEC1 point (65 bytes).
    point: Vec<u8>,
    /// FIDO application (`ssh:` by default) for sk keys.
    pub application: Option<String>,
    pub comment: Option<String>,
}

impl PublicKey {
    /// Parse one `authorized_keys`-style line: `<type> <base64> [comment]` (no options).
    pub fn parse_openssh(line: &str) -> Result<Self> {
        let mut parts = line.trim().splitn(3, char::is_whitespace);
        let ty = parts.next().filter(|s| !s.is_empty()).ok_or_else(|| malformed("empty public key"))?;
        let b64 = parts.next().ok_or_else(|| malformed("public key has no base64 blob"))?;
        let blob = STANDARD.decode(b64.trim()).map_err(|e| malformed(format!("public key base64: {e}")))?;
        let mut k = Self::from_blob(&blob)?;
        if k.key_type.name() != ty {
            return Err(malformed(format!("public key says {ty} but its blob is {}", k.key_type.name())));
        }
        k.comment = parts.next().map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
        Ok(k)
    }

    pub fn from_blob(blob: &[u8]) -> Result<Self> {
        let mut r = Reader(blob);
        let ty = r.string_utf8()?;
        let key_type = KeyType::parse(&ty)?;
        let (point, application) = match key_type {
            KeyType::Ed25519 | KeyType::SkEd25519 => {
                let pk = r.string()?.to_vec();
                if pk.len() != 32 {
                    return Err(malformed("ed25519 public key is not 32 bytes"));
                }
                let app = if key_type.is_sk() { Some(r.string_utf8()?) } else { None };
                (pk, app)
            }
            KeyType::EcdsaP256 | KeyType::SkEcdsaP256 => {
                if r.string()? != b"nistp256" {
                    return Err(malformed("ecdsa key is not on nistp256"));
                }
                let q = r.string()?.to_vec();
                if q.len() != 65 || q[0] != 4 {
                    return Err(malformed("nistp256 point is not uncompressed"));
                }
                let app = if key_type.is_sk() { Some(r.string_utf8()?) } else { None };
                (q, app)
            }
        };
        r.end()?;
        Ok(Self { key_type, blob: blob.to_vec(), point, application, comment: None })
    }

    /// `SHA256:<unpadded base64>`, as `ssh-keygen -lf` prints it.
    pub fn fingerprint(&self) -> String {
        fingerprint_of_blob(&self.blob)
    }

    /// `<type> <base64>` without the comment.
    pub fn to_openssh(&self) -> String {
        format!("{} {}", self.key_type.name(), STANDARD.encode(&self.blob))
    }
}

pub fn fingerprint_of_blob(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

/// A parsed (not yet verified) SSHSIG.
#[derive(Debug, Clone)]
pub struct Signature {
    pub public_key: PublicKey,
    pub namespace: String,
    pub hash_alg: String,
    sig_blob: Vec<u8>,
}

impl Signature {
    /// Parse the `-----BEGIN SSH SIGNATURE-----` armor.
    pub fn parse_armored(armored: &str) -> Result<Self> {
        let s = armored.trim();
        let body = s
            .strip_prefix(BEGIN)
            .and_then(|b| b.strip_suffix(END))
            .ok_or_else(|| malformed("missing BEGIN/END SSH SIGNATURE lines"))?;
        let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let raw = STANDARD.decode(b64).map_err(|e| malformed(format!("base64: {e}")))?;
        Self::parse(&raw)
    }

    pub fn parse(raw: &[u8]) -> Result<Self> {
        let mut r = Reader(raw);
        if r.take(6)? != MAGIC {
            return Err(malformed("not an SSHSIG blob"));
        }
        let version = r.u32()?;
        if version != 1 {
            return Err(malformed(format!("SSHSIG version {version}")));
        }
        let public_key = PublicKey::from_blob(r.string()?)?;
        let namespace = r.string_utf8()?;
        let _reserved = r.string()?;
        let hash_alg = r.string_utf8()?;
        let sig_blob = r.string()?.to_vec();
        r.end()?;
        Ok(Self { public_key, namespace, hash_alg, sig_blob })
    }
}

/// What a successful verification established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub fingerprint: String,
    pub key_type: KeyType,
    pub namespace: String,
    pub hash_alg: String,
    /// sk keys: authenticator flags (0x01 user present, 0x04 user verified) and counter.
    pub sk_flags: Option<u8>,
    pub sk_counter: Option<u32>,
}

/// Verify an armored SSHSIG over `message` with `namespace`. When `expected` is given the
/// signature must be by that key; otherwise the caller must check the returned
/// fingerprint against whatever it trusts.
pub fn verify(armored: &str, namespace: &str, message: &[u8], expected: Option<&PublicKey>) -> Result<Verified> {
    let sig = Signature::parse_armored(armored)?;
    verify_parsed(&sig, namespace, message, expected)
}

pub fn verify_parsed(sig: &Signature, namespace: &str, message: &[u8], expected: Option<&PublicKey>) -> Result<Verified> {
    if sig.namespace != namespace {
        return Err(SigError::WrongNamespace { expected: namespace.to_owned(), found: sig.namespace.clone() });
    }
    let key = &sig.public_key;
    if let Some(e) = expected
        && e.blob != key.blob
    {
        return Err(SigError::WrongKey { expected: e.fingerprint(), found: key.fingerprint() });
    }
    let h: Vec<u8> = match sig.hash_alg.as_str() {
        "sha256" => Sha256::digest(message).to_vec(),
        "sha512" => Sha512::digest(message).to_vec(),
        other => return Err(SigError::UnsupportedHash(other.to_owned())),
    };
    // The bytes the key signed (PROTOCOL.sshsig §"Signed Data").
    let mut data = MAGIC.to_vec();
    put_string(&mut data, sig.namespace.as_bytes());
    put_string(&mut data, b"");
    put_string(&mut data, sig.hash_alg.as_bytes());
    put_string(&mut data, &h);

    let mut r = Reader(&sig.sig_blob);
    let sig_type = r.string_utf8()?;
    if sig_type != key.key_type.name() {
        return Err(malformed(format!("signature type {sig_type} does not match key type {}", key.key_type.name())));
    }
    let raw_sig = r.string()?.to_vec();
    let (signed, sk_flags, sk_counter) = if key.key_type.is_sk() {
        let flags = r.u8()?;
        let counter = r.u32()?;
        let app = key.application.as_deref().unwrap_or_default();
        let mut m = Sha256::digest(app.as_bytes()).to_vec();
        m.push(flags);
        m.extend_from_slice(&counter.to_be_bytes());
        m.extend_from_slice(&Sha256::digest(&data));
        (m, Some(flags), Some(counter))
    } else {
        (data, None, None)
    };
    r.end()?;

    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, ED25519, UnparsedPublicKey};
    let ok = match key.key_type {
        KeyType::Ed25519 | KeyType::SkEd25519 => {
            if raw_sig.len() != 64 {
                return Err(malformed("ed25519 signature is not 64 bytes"));
            }
            UnparsedPublicKey::new(&ED25519, &key.point).verify(&signed, &raw_sig).is_ok()
        }
        KeyType::EcdsaP256 | KeyType::SkEcdsaP256 => {
            let mut rr = Reader(&raw_sig);
            let mut fixed = fixed32(rr.string()?)?;
            fixed.extend_from_slice(&fixed32(rr.string()?)?);
            rr.end()?;
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &key.point).verify(&signed, &fixed).is_ok()
        }
    };
    if !ok {
        return Err(SigError::BadSignature);
    }
    Ok(Verified { fingerprint: key.fingerprint(), key_type: key.key_type, namespace: sig.namespace.clone(), hash_alg: sig.hash_alg.clone(), sk_flags, sk_counter })
}

/// An SSH mpint (positive, big-endian, maybe 0x00-prefixed) as 32 fixed bytes.
fn fixed32(mpint: &[u8]) -> Result<Vec<u8>> {
    let v: &[u8] = {
        let mut i = 0;
        while i < mpint.len() && mpint[i] == 0 {
            i += 1;
        }
        &mpint[i..]
    };
    if v.len() > 32 || mpint.first().is_some_and(|b| b & 0x80 != 0) {
        return Err(malformed("ecdsa signature component out of range"));
    }
    let mut out = vec![0u8; 32 - v.len()];
    out.extend_from_slice(v);
    Ok(out)
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(malformed("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string_utf8(&mut self) -> Result<String> {
        String::from_utf8(self.string()?.to_vec()).map_err(|_| malformed("string is not UTF-8"))
    }
    fn end(&self) -> Result<()> {
        if self.0.is_empty() { Ok(()) } else { Err(malformed("trailing bytes")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fx(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sshsig").join(name)
    }
    fn read(name: &str) -> String {
        std::fs::read_to_string(fx(name)).unwrap()
    }
    fn bytes(name: &str) -> Vec<u8> {
        std::fs::read(fx(name)).unwrap()
    }
    fn key(name: &str) -> PublicKey {
        PublicKey::parse_openssh(&read(name)).unwrap()
    }

    #[test]
    fn fingerprints_match_ssh_keygen() {
        assert_eq!(key("ed25519.pub").fingerprint(), read("ed25519.fp").trim());
        assert_eq!(key("ecdsa.pub").fingerprint(), read("ecdsa.fp").trim());
        assert_eq!(key("openssh/ed25519_sk.pub").fingerprint(), "SHA256:pbI9wlyrFSsUbp+GTiIAOqNMvoajbF2tHsPMECyrMYU");
        assert_eq!(key("openssh/ecdsa_sk.pub").fingerprint(), "SHA256:/ALxJUlmDPE0gxeL1nZip4oXm1VG1qmR2xL5iUzZD1A");
        let k = key("ed25519.pub");
        assert_eq!(k.comment.as_deref(), Some("fixture-ed25519"));
        assert_eq!(PublicKey::parse_openssh(&k.to_openssh()).unwrap().blob, k.blob);
    }

    #[test]
    fn ed25519_and_ecdsa_release_signatures_verify() {
        let msg = bytes("release.jcs.json");
        for (k, sig, alg) in [("ed25519.pub", "release.ed25519.sig", "sha512"), ("ecdsa.pub", "release.ecdsa.sig", "sha512"), ("ecdsa.pub", "release.ecdsa-sha256.sig", "sha256")] {
            let pk = key(k);
            let v = verify(&read(sig), NS_RELEASE, &msg, Some(&pk)).unwrap_or_else(|e| panic!("{sig}: {e}"));
            assert_eq!(v.fingerprint, pk.fingerprint());
            assert_eq!(v.hash_alg, alg);
            assert_eq!(v.sk_flags, None);
            // Without an expected key the signer is still reported.
            assert_eq!(verify(&read(sig), NS_RELEASE, &msg, None).unwrap().fingerprint, pk.fingerprint());
        }
        let v = verify(&read("key-message.ed25519.sig"), NS_KEY, &bytes("key-message.txt"), Some(&key("ed25519.pub"))).unwrap();
        assert_eq!(v.namespace, NS_KEY);
    }

    #[test]
    fn hardware_key_signatures_from_openssh_verify() {
        let ns = read("openssh/namespace");
        let msg = bytes("openssh/signed-data");
        for (k, sig, kt) in [("openssh/ed25519_sk.pub", "openssh/ed25519_sk.sig", KeyType::SkEd25519), ("openssh/ecdsa_sk.pub", "openssh/ecdsa_sk.sig", KeyType::SkEcdsaP256)] {
            let pk = key(k);
            let v = verify(&read(sig), ns.trim(), &msg, Some(&pk)).unwrap_or_else(|e| panic!("{sig}: {e}"));
            assert_eq!((v.key_type, v.fingerprint), (kt, pk.fingerprint()));
            assert!(v.sk_flags.is_some() && v.sk_counter.is_some());
            let mut bad = msg.clone();
            bad[0] ^= 1;
            assert_eq!(verify(&read(sig), ns.trim(), &bad, Some(&pk)), Err(SigError::BadSignature));
        }
    }

    #[test]
    fn wrong_namespace_refused() {
        let msg = bytes("release.jcs.json");
        let e = verify(&read("release.ed25519-git.sig"), NS_RELEASE, &msg, Some(&key("ed25519.pub"))).unwrap_err();
        assert!(matches!(e, SigError::WrongNamespace { .. }), "{e}");
        // A key-registration signature is not a release signature either.
        let e = verify(&read("release.ed25519.sig"), NS_KEY, &msg, None).unwrap_err();
        assert!(matches!(e, SigError::WrongNamespace { .. }), "{e}");
    }

    #[test]
    fn tampered_message_refused() {
        let mut msg = bytes("release.jcs.json");
        let n = msg.len();
        msg[n - 3] = b'1';
        for (k, sig) in [("ed25519.pub", "release.ed25519.sig"), ("ecdsa.pub", "release.ecdsa.sig")] {
            assert_eq!(verify(&read(sig), NS_RELEASE, &msg, Some(&key(k))), Err(SigError::BadSignature), "{sig}");
        }
        // A trailing newline is a different message.
        let mut nl = bytes("release.jcs.json");
        nl.push(b'\n');
        assert_eq!(verify(&read("release.ed25519.sig"), NS_RELEASE, &nl, None), Err(SigError::BadSignature));
    }

    #[test]
    fn wrong_key_refused() {
        let msg = bytes("release.jcs.json");
        let e = verify(&read("release.ed25519.sig"), NS_RELEASE, &msg, Some(&key("ecdsa.pub"))).unwrap_err();
        assert!(matches!(e, SigError::WrongKey { .. }), "{e}");
        // Swapping the embedded key for another one breaks the signature.
        let mut sig = Signature::parse_armored(&read("release.ecdsa.sig")).unwrap();
        let other = key("openssh/ecdsa_sk.pub");
        sig.public_key = PublicKey { key_type: KeyType::EcdsaP256, blob: vec![], point: other.point.clone(), application: None, comment: None };
        assert_eq!(verify_parsed(&sig, NS_RELEASE, &msg, None), Err(SigError::BadSignature));
    }

    #[test]
    fn rsa_and_dsa_refused() {
        let e = PublicKey::parse_openssh(&read("rsa.pub")).unwrap_err();
        assert!(matches!(e, SigError::UnsupportedKey(ref t) if t == "ssh-rsa"), "{e}");
        let e = verify(&read("release.rsa.sig"), NS_RELEASE, &bytes("release.jcs.json"), None).unwrap_err();
        assert!(matches!(e, SigError::UnsupportedKey(_)), "{e}");
        let e = PublicKey::parse_openssh("ssh-dss AAAAB3NzaC1kc3M= x").unwrap_err();
        assert!(matches!(e, SigError::UnsupportedKey(ref t) if t == "ssh-dss"), "{e}");
    }

    #[test]
    fn malformed_input_refused() {
        assert!(matches!(verify("garbage", NS_RELEASE, b"x", None), Err(SigError::Malformed(_))));
        let s = read("release.ed25519.sig");
        let cut = format!("{BEGIN}\n{}\n{END}", &s.lines().nth(1).unwrap()[..20]);
        assert!(matches!(verify(&cut, NS_RELEASE, b"x", None), Err(SigError::Malformed(_))));
        assert!(PublicKey::parse_openssh("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA== x").is_err());
    }
}
