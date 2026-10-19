//! `w3bs publish`: sign package releases with an ordinary SSH key (W3BS-SIGN-1).
//!
//! `w3bs publish key add <namespace>` registers a key for a publisher namespace (a domain):
//! the server hands out a challenge, `ssh-keygen -Y sign` signs it, and the domain proves
//! control with a `.well-known` line or a DNS TXT record. `w3bs publish release` builds the
//! release statement, hashes the installer and artifacts, signs the statement's JCS bytes,
//! checks its own signature and submits it. The private key never leaves `ssh-keygen` (or
//! the ssh-agent); W3BS only ever sees signatures and the public key.

use crate::auth;
use serde_json::{Map, Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use w3bs_core::release::{self, STATEMENT_TYPE};
use w3bs_core::sshsig::{self, PublicKey};
use w3bs_core::{Error, Result};
use w3bs_schema::PackageId;

/// Installer scripts are small; w3bs.org refuses anything larger (W3BS-SIGN-1 §5).
const INSTALLER_LIMIT: usize = 2 * 1024 * 1024;
/// Default keys, in order of preference.
const DEFAULT_KEYS: [&str; 4] = ["id_ed25519", "id_ecdsa", "id_ed25519_sk", "id_ecdsa_sk"];

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("w3bs/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

/// Development transport (tests, a local registry): http on loopback is allowed.
fn dev() -> bool {
    std::env::var("W3BS_DEV_INSECURE_TRANSPORT").is_ok_and(|v| v == "1")
}

fn login_hint(origin: &str) -> String {
    if origin == "https://w3bs.org" { "w3bs login".into() } else { format!("w3bs login --api {origin}") }
}

/// An authenticated call to the publish API. Errors carry the server's `{error:{code,message}}`.
async fn call(origin: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let url = format!("{origin}{path}");
    let mut req = http().request(method, &url).bearer_auth(token);
    if let Some(body) = body {
        req = req.json(body);
    }
    let resp = req.send().await.map_err(|e| Error::package("PUBLISH_UNAVAILABLE", format!("{url}: {e}")).retryable())?;
    let status = resp.status().as_u16();
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default()).unwrap_or(Value::Null);
    match status {
        200..=299 => Ok(v),
        401 => Err(Error::package("NOT_SIGNED_IN", format!("Your session has ended. Run: {}", login_hint(origin)))),
        _ => {
            let msg = v["error"]["message"].as_str().map(str::to_owned).unwrap_or_else(|| format!("the server answered {status}"));
            let code = v["error"]["code"].as_str().map(|c| format!(" ({c})")).unwrap_or_default();
            Err(Error::package("PUBLISH_REJECTED", format!("{msg}{code}")))
        }
    }
}

// ------------------------------------------------------------------ keys and ssh-keygen

/// The key to sign with: what to pass to `ssh-keygen -f`, and its public half.
#[derive(Debug, Clone)]
pub struct SigningKey {
    /// The private key file, or the `.pub` file when the private half lives in ssh-agent.
    pub sign_with: PathBuf,
    pub public: PublicKey,
    /// The `.pub` line as written (type, base64, comment).
    pub public_line: String,
    pub agent: bool,
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| Error::preflight("NO_HOME", "HOME is not set"))
}

fn expand(p: &Path) -> Result<PathBuf> {
    match p.to_str().and_then(|s| s.strip_prefix("~/")) {
        Some(rest) => Ok(home()?.join(rest)),
        None => Ok(p.to_owned()),
    }
}

fn with_pub(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".pub");
    PathBuf::from(s)
}

/// `--key PATH` (private key or `.pub`), else the first of ~/.ssh/id_ed25519, id_ecdsa,
/// id_ed25519_sk, id_ecdsa_sk that exists. When only `<key>.pub` exists, ssh-keygen signs
/// through ssh-agent.
pub fn resolve_key(arg: Option<&Path>) -> Result<SigningKey> {
    let base = match arg {
        Some(p) => {
            let p = expand(p)?;
            match p.to_str().and_then(|s| s.strip_suffix(".pub")) {
                Some(stem) => PathBuf::from(stem),
                None => p,
            }
        }
        None => {
            let ssh = home()?.join(".ssh");
            DEFAULT_KEYS
                .iter()
                .map(|k| ssh.join(k))
                .find(|k| k.exists() || with_pub(k).exists())
                .ok_or_else(|| Error::usage("NO_SSH_KEY", format!("no SSH key in {} ({}); make one with `ssh-keygen -t ed25519` or pass --key", ssh.display(), DEFAULT_KEYS.join(", "))))?
        }
    };
    let pub_path = with_pub(&base);
    let line = std::fs::read_to_string(&pub_path).map_err(|e| Error::usage("NO_SSH_KEY", format!("{}: {e} (the public key must sit next to the private key)", pub_path.display())))?;
    let line = line.lines().find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).unwrap_or_default().trim().to_owned();
    let public = PublicKey::parse_openssh(&line).map_err(|e| Error::usage("UNSUPPORTED_KEY", format!("{}: {e}", pub_path.display())))?;
    let agent = !base.exists();
    Ok(SigningKey { sign_with: if agent { pub_path } else { base }, public, public_line: line, agent })
}

/// `ssh-keygen -Y sign -n <namespace> -f <key>` over `message` (stdin, exactly these bytes).
/// The returned armored signature is verified here before anyone else sees it.
pub fn ssh_sign(key: &SigningKey, namespace: &str, message: &[u8]) -> Result<String> {
    let bin = std::env::var_os("W3BS_SSH_KEYGEN").unwrap_or_else(|| "ssh-keygen".into());
    let mut child = Command::new(&bin)
        .args(["-Y", "sign", "-n", namespace, "-f"])
        .arg(&key.sign_with)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Error::preflight("SSH_KEYGEN", format!("cannot run ssh-keygen (OpenSSH 8.2+ is needed): {e}")))?;
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(message).map_err(|e| Error::package("SSH_KEYGEN", format!("ssh-keygen: {e}")))?;
    drop(stdin);
    let out = child.wait_with_output().map_err(|e| Error::package("SSH_KEYGEN", format!("ssh-keygen: {e}")))?;
    if !out.status.success() {
        let how = if key.agent { " (the private key is not on disk, so it must be loaded in ssh-agent: ssh-add)" } else { "" };
        return Err(Error::package("SIGNING_FAILED", format!("ssh-keygen -Y sign with {} exited with {}{how}", key.sign_with.display(), out.status)));
    }
    let sig = String::from_utf8(out.stdout).map_err(|_| Error::package("SIGNING_FAILED", "ssh-keygen wrote a non-UTF-8 signature"))?;
    sshsig::verify(&sig, namespace, message, Some(&key.public)).map_err(|e| Error::package("SIGNING_FAILED", format!("the signature ssh-keygen made does not verify: {e}")))?;
    Ok(sig.trim_end().to_owned() + "\n")
}

fn check_namespace(ns: &str) -> Result<()> {
    PackageId::new(ns, "x").map(|_| ()).map_err(|e| Error::usage("BAD_NAMESPACE", format!("{ns:?} is not a publisher namespace (a domain such as example.com): {e}")))
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

// ------------------------------------------------------------------ key commands

pub async fn key_add(origin: &str, namespace: &str, key: Option<&Path>) -> Result<Value> {
    check_namespace(namespace)?;
    let key = resolve_key(key)?;
    let ch = call(origin, reqwest::Method::GET, "/api/publish/keys/challenge", None).await?;
    let challenge = ch["challenge"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| Error::package("PUBLISH_REJECTED", "the server sent no challenge"))?;
    let message = ch["message"].as_str().unwrap_or_default();
    // Sign only a key-registration message for this challenge, never arbitrary text.
    if !(message.starts_with("w3bs-key:") && message.ends_with(&format!(":{challenge}")) && !message.contains('\n')) {
        return Err(Error::package("PUBLISH_REJECTED", format!("the server asked to sign {message:?}, which is not a W3BS key-registration message")));
    }
    let signature = ssh_sign(&key, sshsig::NS_KEY, message.as_bytes())?;
    let body = json!({ "public_key": key.public_line, "namespace": namespace, "challenge": challenge, "signature": signature });
    let r = call(origin, reqwest::Method::POST, "/api/publish/keys", Some(&body)).await?;
    let fp = r["fingerprint"].as_str().map(str::to_owned).unwrap_or_else(|| key.public.fingerprint());
    let line = r["proof"]["line"].as_str().map(str::to_owned).unwrap_or_else(|| format!("w3bs-publisher={fp}"));
    let well_known = r["proof"]["well_known"].as_str().map(str::to_owned).unwrap_or_else(|| format!("https://{namespace}/.well-known/w3bs-publisher.txt"));
    let dns = r["proof"]["dns"].as_str().map(str::to_owned).unwrap_or_else(|| format!("_w3bs.{namespace}"));
    Ok(json!({
        "origin": origin, "namespace": namespace, "fingerprint": fp, "key_file": key.sign_with, "agent": key.agent,
        "domain_verified": r["domain_verified"] == true,
        "proof": { "well_known": well_known, "dns": dns, "line": line },
        "next": if r["domain_verified"] == true { Value::Null } else { json!(format!("w3bs publish key verify {fp}")) },
    }))
}

pub async fn key_ls(origin: &str) -> Result<Value> {
    let r = call(origin, reqwest::Method::GET, "/api/publish/keys", None).await?;
    Ok(json!({ "origin": origin, "keys": r["keys"].as_array().cloned().unwrap_or_default() }))
}

/// Re-check the domain proof of one key, or of every unrevoked key when none is named.
pub async fn key_verify(origin: &str, fingerprint: Option<&str>) -> Result<Value> {
    let fps: Vec<String> = match fingerprint {
        Some(f) => vec![f.to_owned()],
        None => key_ls(origin).await?["keys"]
            .as_array()
            .map(|a| a.iter().filter(|k| k["revoked_at"].is_null()).filter_map(|k| k["fingerprint"].as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
    };
    if fps.is_empty() {
        return Err(Error::usage("NO_KEYS", "no registered keys; add one with `w3bs publish key add <namespace>`"));
    }
    let mut keys = vec![];
    for fp in fps {
        keys.push(call(origin, reqwest::Method::POST, "/api/publish/keys/verify", Some(&json!({ "fingerprint": fp }))).await?);
    }
    Ok(json!({ "origin": origin, "keys": keys }))
}

pub async fn key_revoke(origin: &str, fingerprint: &str) -> Result<Value> {
    if !fingerprint.starts_with("SHA256:") {
        return Err(Error::usage("BAD_FINGERPRINT", "give the key's SHA256:… fingerprint (see `w3bs publish key ls`)"));
    }
    let r = call(origin, reqwest::Method::DELETE, &format!("/api/publish/keys/{}", url_encode(fingerprint)), None).await?;
    Ok(json!({ "origin": origin, "fingerprint": fingerprint, "revoked": true, "response": r }))
}

// ------------------------------------------------------------------ releases

#[derive(Debug, Clone, Default)]
pub struct ReleaseArgs {
    pub package: Option<String>,
    pub version: Option<String>,
    pub install: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub commands: Vec<String>,
    /// `target=url`
    pub artifacts: Vec<String>,
    pub key: Option<PathBuf>,
    pub from: Option<PathBuf>,
    pub dry_run: bool,
}

/// `w3bs.toml` (or `[package]` inside it): the same fields as the flags. Flags win.
fn merge_file(a: &mut ReleaseArgs, path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::usage("READ", format!("{}: {e}", path.display())))?;
    let doc: toml::Table = toml::from_str(&text).map_err(|e| Error::usage("INVALID_TOML", format!("{}: {e}", path.display())))?;
    let t = match doc.get("package") {
        Some(toml::Value::Table(p)) => p.clone(),
        _ => doc.clone(),
    };
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let fill = |slot: &mut Option<String>, v: Option<String>| {
        if slot.is_none() {
            *slot = v;
        }
    };
    fill(&mut a.package, s("id").or_else(|| s("package")));
    fill(&mut a.version, s("version"));
    fill(&mut a.install, s("install"));
    fill(&mut a.name, s("name"));
    fill(&mut a.description, s("description"));
    fill(&mut a.homepage, s("homepage"));
    fill(&mut a.repository, s("repository"));
    fill(&mut a.license, s("license"));
    if a.commands.is_empty()
        && let Some(c) = t.get("commands").and_then(|v| v.as_array())
    {
        a.commands = c.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
    }
    if a.artifacts.is_empty() {
        let arts = t.get("artifacts").or_else(|| doc.get("artifacts"));
        match arts {
            Some(toml::Value::Array(list)) => {
                for x in list {
                    let (Some(target), Some(url)) = (x.get("target").and_then(|v| v.as_str()), x.get("url").and_then(|v| v.as_str())) else {
                        return Err(Error::usage("INVALID_TOML", format!("{}: each [[artifacts]] needs target and url", path.display())));
                    };
                    a.artifacts.push(format!("{target}={url}"));
                }
            }
            Some(toml::Value::Table(map)) => {
                for (target, url) in map {
                    a.artifacts.push(format!("{target}={}", url.as_str().unwrap_or_default()));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The package's current listing, when it has one (defaults for the statement).
async fn existing_entry(origin: &str, id: &PackageId) -> Option<Value> {
    let url = format!("{origin}/api/packages/{}/{}", id.publisher(), id.name());
    let r = http().get(&url).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let v: Value = r.json().await.ok()?;
    let v = if v["package"].is_object() { v["package"].clone() } else { v };
    (v["id"].as_str() == Some(&id.to_string())).then_some(v)
}

/// Download `url` and return its sha256 (hex), refusing anything but https (loopback http
/// in development).
async fn sha256_of(url: &str, limit: Option<usize>) -> Result<String> {
    use futures::StreamExt;
    use sha2::{Digest, Sha256};
    let parsed = url::Url::parse(url).map_err(|e| Error::usage("INVALID_URL", format!("{url}: {e}")))?;
    let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if parsed.scheme() != "https" && !(dev() && parsed.scheme() == "http" && loopback) {
        return Err(Error::usage("INSECURE_URL", format!("{url} must be https://")));
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(600)).user_agent(concat!("w3bs/", env!("CARGO_PKG_VERSION"))).build().expect("http client");
    let resp = client.get(parsed).send().await.and_then(|r| r.error_for_status()).map_err(|e| Error::package("DOWNLOAD_FAILED", format!("{url}: {e}")).retryable())?;
    let mut h = Sha256::new();
    let mut n = 0usize;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::package("DOWNLOAD_FAILED", format!("{url}: {e}")).retryable())?;
        n += chunk.len();
        if limit.is_some_and(|l| n > l) {
            return Err(Error::package("INSTALLER_TOO_LARGE", format!("{url} is over {} MiB; installers must be small scripts", INSTALLER_LIMIT / 1024 / 1024)));
        }
        h.update(&chunk);
    }
    if n == 0 {
        return Err(Error::package("DOWNLOAD_FAILED", format!("{url} is empty")));
    }
    Ok(hex::encode(h.finalize()))
}

/// Statement fields from flags, the file and the current listing, before hashing.
pub fn draft_statement(a: &ReleaseArgs, existing: Option<&Value>, created_at: &str) -> Result<(PackageId, Map<String, Value>)> {
    let raw = a.package.as_deref().ok_or_else(|| Error::usage("MISSING_PACKAGE", "give --package <namespace/name> (or `id` in --from)"))?;
    let id: PackageId = raw.parse().map_err(|e| Error::usage("INVALID_PACKAGE", format!("{raw}: {e}")))?;
    let version = a.version.as_deref().ok_or_else(|| Error::usage("MISSING_VERSION", "give --version <semver>"))?;
    semver::Version::parse(version).map_err(|e| Error::usage("INVALID_VERSION", format!("{version}: {e}")))?;
    let ex = |k: &str| existing.and_then(|e| e[k].as_str()).filter(|s| !s.is_empty()).map(str::to_owned);
    let install = a.install.clone().or_else(|| ex("install")).ok_or_else(|| Error::usage("MISSING_INSTALL", "give --install \"curl -fsSL https://…/install.sh | sh\""))?;
    let installer_url = release::install_line_url(&install).ok_or_else(|| Error::usage("BAD_INSTALL", format!("--install must be `curl -fsSL https://… | sh`, not {install:?}")))?.to_owned();
    let description = a.description.clone().or_else(|| ex("description")).ok_or_else(|| Error::usage("MISSING_DESCRIPTION", "give --description (the package has no listing to take it from)"))?;
    let mut st = Map::new();
    st.insert("type".into(), json!(STATEMENT_TYPE));
    st.insert("package".into(), json!(id.to_string()));
    st.insert("version".into(), json!(version));
    st.insert("name".into(), json!(a.name.clone().or_else(|| ex("name")).unwrap_or_else(|| id.name().to_owned())));
    st.insert("description".into(), json!(description));
    for (k, v) in [("homepage", &a.homepage), ("repository", &a.repository), ("license", &a.license)] {
        if let Some(v) = v.clone().or_else(|| ex(k)) {
            st.insert(k.into(), json!(v));
        }
    }
    let commands: Vec<String> = if a.commands.is_empty() {
        existing.and_then(|e| e["commands"].as_array()).map(|c| c.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    } else {
        a.commands.clone()
    };
    if !commands.is_empty() {
        st.insert("commands".into(), json!(commands));
    }
    st.insert("install".into(), json!(install));
    st.insert("installer".into(), json!({ "url": installer_url }));
    if !a.artifacts.is_empty() {
        let mut arts = vec![];
        for spec in &a.artifacts {
            let (target, url) = spec.split_once('=').filter(|(t, u)| !t.is_empty() && !u.is_empty()).ok_or_else(|| Error::usage("BAD_ARTIFACT", format!("--artifact {spec:?}: use target=https://…")))?;
            arts.push(json!({ "target": target, "url": url }));
        }
        st.insert("artifacts".into(), Value::Array(arts));
    }
    st.insert("created_at".into(), json!(created_at));
    Ok((id, st))
}

pub async fn release(origin: &str, mut a: ReleaseArgs) -> Result<Value> {
    if let Some(f) = a.from.clone() {
        merge_file(&mut a, &f)?;
    }
    // Validate what we can before any network or key use.
    let raw = a.package.as_deref().ok_or_else(|| Error::usage("MISSING_PACKAGE", "give --package <namespace/name> (or `id` in --from)"))?;
    let pre: PackageId = raw.parse().map_err(|e| Error::usage("INVALID_PACKAGE", format!("{raw}: {e}")))?;
    let key = resolve_key(a.key.as_deref())?;
    let existing = existing_entry(origin, &pre).await;
    let (id, mut st) = draft_statement(&a, existing.as_ref(), &w3bs_core::now())?;
    let installer_url = st["installer"]["url"].as_str().unwrap_or_default().to_owned();
    let sha = sha256_of(&installer_url, Some(INSTALLER_LIMIT)).await?;
    st["installer"]["sha256"] = json!(sha);
    if let Some(Value::Array(arts)) = st.get_mut("artifacts") {
        for art in arts.iter_mut() {
            let u = art["url"].as_str().unwrap_or_default().to_owned();
            art["sha256"] = json!(sha256_of(&u, None).await?);
        }
    }
    let statement = Value::Object(st);
    release::check_statement_with(&statement, dev()).map_err(|e| Error::usage("INVALID_STATEMENT", e))?;
    let bytes = release::statement_bytes(&statement)?;
    let signature = ssh_sign(&key, sshsig::NS_RELEASE, &bytes)?;
    let fingerprint = key.public.fingerprint();
    let mut out = json!({
        "origin": origin, "package": id.to_string(), "version": statement["version"], "fingerprint": fingerprint,
        "public_key": key.public.to_openssh(), "dry_run": a.dry_run, "defaults_from_listing": existing.is_some(),
        "statement": statement, "signature": signature,
    });
    if !a.dry_run {
        let r = call(origin, reqwest::Method::POST, "/api/publish/releases", Some(&json!({ "statement": out["statement"], "signature": out["signature"] }))).await?;
        out["url"] = r["url"].clone();
        out["response"] = r;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_from_flags_and_listing() {
        let listing = json!({ "id": "pairux.com/pairux", "name": "pairux", "description": "Screen sharing", "install": "curl -fsSL https://pairux.com/install.sh | sh", "commands": ["pairux"], "license": "MIT" });
        let a = ReleaseArgs { package: Some("pairux.com/pairux".into()), version: Some("0.17.0".into()), homepage: Some("https://pairux.com".into()), ..Default::default() };
        let (id, st) = draft_statement(&a, Some(&listing), "2026-10-19T20:00:00Z").unwrap();
        assert_eq!(id.to_string(), "pairux.com/pairux");
        assert_eq!(st["installer"]["url"], "https://pairux.com/install.sh");
        assert_eq!((st["commands"].clone(), st["license"].clone(), st["homepage"].clone()), (json!(["pairux"]), json!("MIT"), json!("https://pairux.com")));
        let mut v = Value::Object(st);
        v["installer"]["sha256"] = json!("a".repeat(64));
        release::check_statement(&v).unwrap();
        // Without a listing, install and description are required.
        assert_eq!(draft_statement(&a, None, "2026-10-19T20:00:00Z").unwrap_err().code, "MISSING_INSTALL");
        let bad = ReleaseArgs { install: Some("wget x | sh".into()), ..a.clone() };
        assert_eq!(draft_statement(&bad, Some(&listing), "t").unwrap_err().code, "BAD_INSTALL");
        let bad = ReleaseArgs { version: Some("1.0".into()), ..a.clone() };
        assert_eq!(draft_statement(&bad, Some(&listing), "t").unwrap_err().code, "INVALID_VERSION");
        let arts = ReleaseArgs { artifacts: vec!["linux-x86_64=https://x.example/a".into(), "nope".into()], ..a };
        assert_eq!(draft_statement(&arts, Some(&listing), "t").unwrap_err().code, "BAD_ARTIFACT");
    }

    #[test]
    fn w3bs_toml_fills_unset_flags() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w3bs.toml");
        std::fs::write(
            &p,
            "[package]\nid = \"pairux.com/pairux\"\nversion = \"1.0.0\"\ndescription = \"d\"\ninstall = \"curl -fsSL https://pairux.com/install.sh | sh\"\ncommands = [\"pairux\"]\n\n[[artifacts]]\ntarget = \"linux-x86_64\"\nurl = \"https://pairux.com/a\"\n",
        )
        .unwrap();
        let mut a = ReleaseArgs { version: Some("2.0.0".into()), ..Default::default() };
        merge_file(&mut a, &p).unwrap();
        assert_eq!((a.package.as_deref(), a.version.as_deref()), (Some("pairux.com/pairux"), Some("2.0.0")));
        assert_eq!(a.commands, vec!["pairux"]);
        assert_eq!(a.artifacts, vec!["linux-x86_64=https://pairux.com/a"]);
    }

    #[test]
    fn key_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../w3bs-core/tests/fixtures/sshsig");
        // Only the .pub on disk: sign through ssh-agent with -f <key>.pub.
        std::fs::copy(fx.join("ed25519.pub"), dir.path().join("id_ed25519.pub")).unwrap();
        let k = resolve_key(Some(&dir.path().join("id_ed25519"))).unwrap();
        assert!(k.agent);
        assert_eq!(k.sign_with, dir.path().join("id_ed25519.pub"));
        assert_eq!(k.public.fingerprint(), std::fs::read_to_string(fx.join("ed25519.fp")).unwrap().trim());
        std::fs::write(dir.path().join("id_ed25519"), "private").unwrap();
        let k = resolve_key(Some(&dir.path().join("id_ed25519.pub"))).unwrap();
        assert!(!k.agent);
        assert_eq!(k.sign_with, dir.path().join("id_ed25519"));
        std::fs::copy(fx.join("rsa.pub"), dir.path().join("id_rsa.pub")).unwrap();
        assert_eq!(resolve_key(Some(&dir.path().join("id_rsa"))).unwrap_err().code, "UNSUPPORTED_KEY");
    }

    #[test]
    fn fingerprints_are_path_encoded() {
        assert_eq!(url_encode("SHA256:ab+c/d="), "SHA256%3Aab%2Bc%2Fd%3D");
    }
}
