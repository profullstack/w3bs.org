//! `w3bs login`, `w3bs logout`, `w3bs whoami`: a W3BS account on this machine.
//!
//! The CLI is a public OAuth 2.1 client (`w3bs-cli`): authorization code with PKCE S256,
//! redirected to a one-shot loopback listener (RFC 8252), refresh tokens rotated on every
//! use. Credentials live in `$XDG_CONFIG_HOME/w3bs/auth.json` (0600), keyed by origin, so
//! one machine can be signed in to w3bs.org and a local registry at the same time.
//! Later commands that call an authenticated API take a token from [`access_token`].

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use w3bs_core::{Error, Result};

pub const CLIENT_ID: &str = "w3bs-cli";
const SCOPE: &str = "profile";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh this long before the access token's stated expiry.
const EXPIRY_SLACK: u64 = 30;

// ------------------------------------------------------------------ origin + discovery

/// The account origin: `--api`, else `$W3BS_API`, else https://w3bs.org. Same rules as the
/// resource registry: an origin only, HTTPS except for loopback development.
pub fn origin(api: Option<&str>) -> Result<String> {
    let raw = api.map(str::to_owned).or_else(|| std::env::var("W3BS_API").ok()).unwrap_or_else(|| "https://w3bs.org".into());
    let url = url::Url::parse(&raw).map_err(|e| Error::usage("INVALID_API", format!("{raw}: {e}")))?;
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return Err(Error::usage("INVALID_API", "the account origin must be an origin without credentials, path, query or fragment"));
    }
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback(&url)) {
        return Err(Error::usage("HTTPS_REQUIRED", "use HTTPS except for loopback development"));
    }
    Ok(url.origin().ascii_serialization())
}

fn is_loopback(url: &url::Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub authorization: String,
    pub token: String,
    pub revocation: String,
    pub userinfo: String,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

fn net_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::package("AUTH_UNAVAILABLE", format!("{what}: {e}")).retryable()
}

/// RFC 8414 metadata. Every endpoint must sit on the origin itself, so a tampered or
/// misconfigured document cannot send codes or refresh tokens anywhere else.
pub async fn discover(client: &reqwest::Client, origin: &str) -> Result<Endpoints> {
    let url = format!("{origin}/.well-known/oauth-authorization-server");
    let res = client.get(&url).send().await.map_err(|e| net_err(&url, e))?;
    if !res.status().is_success() {
        return Err(Error::package("AUTH_UNSUPPORTED", format!("{origin} does not offer W3BS accounts ({url}: HTTP {})", res.status().as_u16())));
    }
    let v: Value = res.json().await.map_err(|e| net_err(&url, e))?;
    let field = |name: &str| -> Result<String> {
        let s = v[name].as_str().ok_or_else(|| Error::package("AUTH_UNSUPPORTED", format!("{url} has no {name}")))?;
        let u = url::Url::parse(s).map_err(|e| Error::package("AUTH_UNSUPPORTED", format!("{name} {s}: {e}")))?;
        if u.origin().ascii_serialization() != origin {
            return Err(Error::package("AUTH_UNSUPPORTED", format!("{name} {s} is not on {origin}")));
        }
        Ok(s.to_owned())
    };
    Ok(Endpoints { authorization: field("authorization_endpoint")?, token: field("token_endpoint")?, revocation: field("revocation_endpoint")?, userinfo: field("userinfo_endpoint")? })
}

// ------------------------------------------------------------------ PKCE

/// Unpadded base64url (RFC 4648 §5).
pub fn base64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// 32 random bytes as base64url: a PKCE verifier (43 characters) or a state value.
fn random_token() -> Result<String> {
    use aws_lc_rs::rand::{SecureRandom, SystemRandom};
    let mut b = [0u8; 32];
    SystemRandom::new().fill(&mut b).map_err(|_| Error::preflight("RANDOM", "the system random source failed"))?;
    Ok(base64url(&b))
}

// ------------------------------------------------------------------ loopback redirect

#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code { code: String, state: String },
    Denied { error: String, description: String, state: String },
}

/// Parse an HTTP request line. `None` for anything that is not the callback (a favicon
/// request, a stray probe), which gets a 404 while the listener keeps waiting.
pub fn parse_callback(request_line: &str) -> Option<Callback> {
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        return None;
    }
    let mut q: Map<String, Value> = Map::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        q.entry(k.into_owned()).or_insert(Value::String(v.into_owned()));
    }
    let get = |k: &str| q.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    if q.contains_key("error") {
        return Some(Callback::Denied { error: get("error"), description: get("error_description"), state: get("state") });
    }
    if q.contains_key("code") {
        return Some(Callback::Code { code: get("code"), state: get("state") });
    }
    None
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let page = format!("<!doctype html><meta charset=utf-8><title>W3BS</title><body style=\"font:16px system-ui;margin:4rem auto;max-width:32rem\"><p>{body}</p>");
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}", page.len());
    let _ = stream.flush();
}

fn read_request_line(stream: &mut TcpStream) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    text.lines().next().map(str::to_owned)
}

/// Accept connections until the callback arrives with our `state`, or the deadline passes.
pub fn wait_for_callback(listener: &TcpListener, state: &str, timeout: Duration) -> Result<String> {
    listener.set_nonblocking(true).map_err(|e| Error::preflight("LOOPBACK", e.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let Some(line) = read_request_line(&mut stream) else { continue };
                match parse_callback(&line) {
                    None => respond(&mut stream, "404 Not Found", "Not found."),
                    Some(Callback::Code { state: s, .. } | Callback::Denied { state: s, .. }) if s != state => {
                        respond(&mut stream, "400 Bad Request", "This sign-in link does not belong to the waiting <code>w3bs login</code>. Run it again.");
                        return Err(Error::package("STATE_MISMATCH", "the authorization response did not carry this login's state; run `w3bs login` again"));
                    }
                    Some(Callback::Denied { error, description, .. }) => {
                        respond(&mut stream, "200 OK", "Sign-in was cancelled. You can close this tab.");
                        let msg = if description.is_empty() { error } else { format!("{error}: {description}") };
                        return Err(Error::package("LOGIN_DENIED", msg));
                    }
                    Some(Callback::Code { code, .. }) => {
                        respond(&mut stream, "200 OK", "Signed in to W3BS. You can close this tab.");
                        return Ok(code);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::package("LOGIN_TIMEOUT", "no sign-in completed within 5 minutes; run `w3bs login` again"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(Error::preflight("LOOPBACK", e.to_string())),
        }
    }
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(windows)]
    let mut cmd = {
        // `cmd /c start` would split the URL at every `&`.
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

// ------------------------------------------------------------------ token endpoint

#[derive(Debug, Clone, PartialEq)]
pub struct Creds {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub username: String,
}

impl Creds {
    fn to_json(&self) -> Value {
        json!({ "access_token": self.access_token, "refresh_token": self.refresh_token, "expires_at": self.expires_at, "username": self.username })
    }
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            access_token: v["access_token"].as_str()?.to_owned(),
            refresh_token: v["refresh_token"].as_str()?.to_owned(),
            expires_at: v["expires_at"].as_u64().unwrap_or(0),
            username: v["username"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

async fn post_form(client: &reqwest::Client, url: &str, pairs: &[(&str, &str)]) -> Result<reqwest::Response> {
    client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(reqwest::header::ACCEPT, "application/json")
        .body(form(pairs))
        .send()
        .await
        .map_err(|e| net_err(url, e))
}

/// A token response, or the OAuth error it carries (`invalid_grant` means sign in again).
async fn token_request(client: &reqwest::Client, ep: &Endpoints, pairs: &[(&str, &str)], username: &str) -> Result<Creds> {
    let res = post_form(client, &ep.token, pairs).await?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let error = v["error"].as_str().unwrap_or("server_error");
        let desc = v["error_description"].as_str().unwrap_or_default();
        let code = if error == "invalid_grant" { "SESSION_EXPIRED" } else { "OAUTH_ERROR" };
        return Err(Error::package(code, if desc.is_empty() { format!("{error} (HTTP {})", status.as_u16()) } else { format!("{error}: {desc}") }));
    }
    let field = |k: &str| v[k].as_str().map(str::to_owned).ok_or_else(|| Error::package("OAUTH_ERROR", format!("token response has no {k}")));
    if !v["token_type"].as_str().unwrap_or("Bearer").eq_ignore_ascii_case("bearer") {
        return Err(Error::package("OAUTH_ERROR", "token endpoint returned a non-Bearer token"));
    }
    Ok(Creds { access_token: field("access_token")?, refresh_token: field("refresh_token")?, expires_at: now() + v["expires_in"].as_u64().unwrap_or(3600), username: username.to_owned() })
}

async fn refresh(client: &reqwest::Client, ep: &Endpoints, c: &Creds) -> Result<Creds> {
    token_request(client, ep, &[("grant_type", "refresh_token"), ("refresh_token", &c.refresh_token), ("client_id", CLIENT_ID)], &c.username).await
}

enum Userinfo {
    Ok(Value),
    Unauthorized,
}

async fn userinfo(client: &reqwest::Client, ep: &Endpoints, access: &str) -> Result<Userinfo> {
    let res = client.get(&ep.userinfo).bearer_auth(access).send().await.map_err(|e| net_err(&ep.userinfo, e))?;
    if res.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(Userinfo::Unauthorized);
    }
    if !res.status().is_success() {
        return Err(Error::package("USERINFO_FAILED", format!("{}: HTTP {}", ep.userinfo, res.status().as_u16())));
    }
    Ok(Userinfo::Ok(res.json().await.map_err(|e| net_err(&ep.userinfo, e))?))
}

// ------------------------------------------------------------------ credential store

/// `$XDG_CONFIG_HOME/w3bs/auth.json`, else `~/.config/w3bs/auth.json`.
pub fn auth_path() -> Result<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Ok(PathBuf::from(x).join("w3bs/auth.json"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or_else(|| Error::preflight("NO_HOME", "neither XDG_CONFIG_HOME nor HOME is set"))?;
    Ok(PathBuf::from(home).join(".config/w3bs/auth.json"))
}

pub fn load(path: &Path) -> Result<Map<String, Value>> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(Error::preflight("AUTH_FILE", format!("{} is not a JSON object; remove it and run `w3bs login`", path.display()))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(Error::preflight("AUTH_FILE", format!("{}: {e}", path.display()))),
    }
}

/// The directory holding auth.json, created 0700 when missing.
fn config_dir(path: &Path) -> Result<&Path> {
    let dir = path.parent().ok_or_else(|| Error::preflight("AUTH_FILE", "auth file has no directory"))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|e| Error::preflight("AUTH_FILE", format!("{}: {e}", dir.display())))?;
    Ok(dir)
}

/// Write the whole file atomically (temp file in the same directory, then rename), 0600.
pub fn store(path: &Path, all: &Map<String, Value>) -> Result<()> {
    let io = |e: std::io::Error| Error::preflight("AUTH_FILE", format!("{}: {e}", path.display()));
    let dir = config_dir(path)?;
    let tmp = dir.join(format!(".auth.json.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&Value::Object(all.clone())).expect("json").as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// Serialize read-refresh-write across concurrent `w3bs` processes: refresh tokens rotate,
/// and two processes refreshing the same one would trip the server's reuse detection.
fn lock(path: &Path) -> Result<std::fs::File> {
    use fs4::fs_std::FileExt;
    let dir = config_dir(path)?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("auth.lock")).map_err(|e| Error::preflight("AUTH_FILE", e.to_string()))?;
    f.lock_exclusive().map_err(|e| Error::preflight("AUTH_FILE", e.to_string()))?;
    Ok(f)
}

fn save_creds(path: &Path, origin: &str, c: &Creds) -> Result<()> {
    let mut all = load(path)?;
    all.insert(origin.to_owned(), c.to_json());
    store(path, &all)
}

fn not_signed_in(origin: &str) -> Error {
    let hint = if origin == "https://w3bs.org" { "w3bs login".to_owned() } else { format!("w3bs login --api {origin}") };
    Error::package("NOT_SIGNED_IN", format!("Not signed in. Run: {hint}"))
}

// ------------------------------------------------------------------ commands

pub async fn login(origin: &str, browser: bool, progress: &dyn Fn(&str)) -> Result<Value> {
    let client = http();
    let ep = discover(&client, origin).await?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| Error::preflight("LOOPBACK", format!("cannot listen on 127.0.0.1: {e}")))?;
    let port = listener.local_addr().map_err(|e| Error::preflight("LOOPBACK", e.to_string()))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let verifier = random_token()?;
    let state = random_token()?;
    let mut url = url::Url::parse(&ep.authorization).map_err(|e| Error::package("AUTH_UNSUPPORTED", e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    let url = url.to_string();
    if browser && open_browser(&url) {
        progress(&format!("opened your browser to sign in to {origin}; if nothing appeared, visit:\n  {url}"));
    } else {
        // Always shown: without a browser this URL is the only way forward.
        eprintln!("w3bs: to sign in to {origin}, open this URL in a browser on this machine:\n  {url}");
    }
    progress("waiting for the browser (5 minutes)…");
    let st = state.clone();
    let code = tokio::task::spawn_blocking(move || wait_for_callback(&listener, &st, LOGIN_TIMEOUT))
        .await
        .map_err(|e| Error::preflight("LOOPBACK", e.to_string()))??;
    let mut creds = token_request(
        &client,
        &ep,
        &[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", &redirect_uri), ("client_id", CLIENT_ID), ("code_verifier", &verifier)],
        "",
    )
    .await?;
    let me = match userinfo(&client, &ep, &creds.access_token).await? {
        Userinfo::Ok(v) => v,
        Userinfo::Unauthorized => return Err(Error::package("OAUTH_ERROR", "the new access token was refused by the userinfo endpoint")),
    };
    creds.username = me["username"].as_str().unwrap_or_default().to_owned();
    let path = auth_path()?;
    let _guard = lock(&path)?;
    save_creds(&path, origin, &creds)?;
    Ok(json!({ "origin": origin, "user": me, "credentials": path }))
}

/// A valid access token for `origin`, refreshing (and saving the rotated refresh token)
/// when it is about to expire. Errors with NOT_SIGNED_IN when there is no session.
#[allow(dead_code)] // the entry point for authenticated commands; none ship yet
pub async fn access_token(origin: &str) -> Result<String> {
    let path = auth_path()?;
    let _guard = lock(&path)?;
    let c = load(&path)?.get(origin).and_then(Creds::from_json).ok_or_else(|| not_signed_in(origin))?;
    if c.expires_at > now() + EXPIRY_SLACK {
        return Ok(c.access_token);
    }
    let client = http();
    let ep = discover(&client, origin).await?;
    Ok(refresh_and_save(&client, &ep, &path, origin, &c).await?.access_token)
}

async fn refresh_and_save(client: &reqwest::Client, ep: &Endpoints, path: &Path, origin: &str, c: &Creds) -> Result<Creds> {
    match refresh(client, ep, c).await {
        Ok(n) => {
            save_creds(path, origin, &n)?;
            Ok(n)
        }
        Err(e) if e.code == "SESSION_EXPIRED" => {
            let mut all = load(path)?;
            all.remove(origin);
            store(path, &all)?;
            Err(Error::package("NOT_SIGNED_IN", format!("{} Run: w3bs login{}", "Your session has ended.", if origin == "https://w3bs.org" { String::new() } else { format!(" --api {origin}") })))
        }
        Err(e) => Err(e),
    }
}

pub async fn whoami(origin: &str) -> Result<Value> {
    let path = auth_path()?;
    let _guard = lock(&path)?;
    let mut c = load(&path)?.get(origin).and_then(Creds::from_json).ok_or_else(|| not_signed_in(origin))?;
    let client = http();
    let ep = discover(&client, origin).await?;
    if c.expires_at <= now() + EXPIRY_SLACK {
        c = refresh_and_save(&client, &ep, &path, origin, &c).await?;
    }
    let me = match userinfo(&client, &ep, &c.access_token).await? {
        Userinfo::Ok(v) => v,
        Userinfo::Unauthorized => {
            c = refresh_and_save(&client, &ep, &path, origin, &c).await?;
            match userinfo(&client, &ep, &c.access_token).await? {
                Userinfo::Ok(v) => v,
                Userinfo::Unauthorized => return Err(not_signed_in(origin)),
            }
        }
    };
    if let Some(u) = me["username"].as_str().filter(|u| *u != c.username) {
        c.username = u.to_owned();
        save_creds(&path, origin, &c)?;
    }
    Ok(json!({ "origin": origin, "user": me }))
}

pub async fn logout(origin: &str) -> Result<Value> {
    let path = auth_path()?;
    let _guard = lock(&path)?;
    let mut all = load(&path)?;
    let Some(c) = all.remove(origin).as_ref().and_then(Creds::from_json) else {
        return Ok(json!({ "origin": origin, "signedIn": false }));
    };
    // Revoke first so the server ends the session, but never let a network problem keep
    // the token on disk: the user asked for it to be gone.
    let client = http();
    let revoked = match discover(&client, origin).await {
        Ok(ep) => post_form(&client, &ep.revocation, &[("token", &c.refresh_token), ("token_type_hint", "refresh_token"), ("client_id", CLIENT_ID)]).await.map(|r| r.status().is_success()).unwrap_or(false),
        Err(_) => false,
    };
    store(&path, &all)?;
    Ok(json!({ "origin": origin, "signedIn": true, "revoked": revoked }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_appendix_b() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn base64url_is_unpadded_and_url_safe() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(&[0xfb, 0xff, 0xbf]), "-_-_");
        // RFC 7636 appendix B: the verifier is the base64url of these 32 octets.
        let octets = [116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121];
        assert_eq!(base64url(&octets), "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    }

    #[test]
    fn random_tokens_are_43_chars_and_distinct() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
    }

    #[test]
    fn parses_callback_request_lines() {
        assert_eq!(
            parse_callback("GET /callback?code=abc%2B1&state=xyz HTTP/1.1"),
            Some(Callback::Code { code: "abc+1".into(), state: "xyz".into() })
        );
        assert_eq!(
            parse_callback("GET /callback?error=access_denied&error_description=User+said+no&state=s HTTP/1.1"),
            Some(Callback::Denied { error: "access_denied".into(), description: "User said no".into(), state: "s".into() })
        );
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1"), None);
        assert_eq!(parse_callback("POST /callback?code=a&state=b HTTP/1.1"), None);
        assert_eq!(parse_callback("GET /callback HTTP/1.1"), None);
        assert_eq!(parse_callback(""), None);
    }

    #[test]
    fn origin_rules() {
        assert_eq!(origin(Some("https://w3bs.org")).unwrap(), "https://w3bs.org");
        assert_eq!(origin(Some("https://w3bs.org/")).unwrap(), "https://w3bs.org");
        assert_eq!(origin(Some("http://127.0.0.1:3000")).unwrap(), "http://127.0.0.1:3000");
        assert_eq!(origin(Some("http://w3bs.org")).unwrap_err().code, "HTTPS_REQUIRED");
        assert_eq!(origin(Some("https://w3bs.org/api")).unwrap_err().code, "INVALID_API");
        assert_eq!(origin(Some("https://u:p@w3bs.org")).unwrap_err().code, "INVALID_API");
    }

    #[test]
    fn auth_file_round_trip_is_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/w3bs/auth.json");
        assert!(load(&path).unwrap().is_empty());
        let c = Creds { access_token: "at".into(), refresh_token: "rt".into(), expires_at: 42, username: "riotcoder".into() };
        save_creds(&path, "https://w3bs.org", &c).unwrap();
        save_creds(&path, "http://127.0.0.1:3000", &Creds { username: "local".into(), ..c.clone() }).unwrap();
        let all = load(&path).unwrap();
        assert_eq!(Creds::from_json(&all["https://w3bs.org"]), Some(c));
        assert_eq!(all["http://127.0.0.1:3000"]["username"], "local");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    fn get(port: u16, target: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn loopback_listener_skips_strays_and_returns_the_code() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let stray = get(port, "/favicon.ico");
            let ok = get(port, "/callback?code=the-code&state=st");
            (stray, ok)
        });
        assert_eq!(wait_for_callback(&listener, "st", Duration::from_secs(10)).unwrap(), "the-code");
        let (stray, ok) = client.join().unwrap();
        assert!(stray.starts_with("HTTP/1.1 404"));
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("Signed in to W3BS"));
    }

    #[test]
    fn loopback_listener_rejects_a_foreign_state() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || get(port, "/callback?code=c&state=other"));
        assert_eq!(wait_for_callback(&listener, "mine", Duration::from_secs(10)).unwrap_err().code, "STATE_MISMATCH");
        assert!(client.join().unwrap().starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn loopback_listener_reports_denial() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || get(port, "/callback?error=access_denied&state=s"));
        let e = wait_for_callback(&listener, "s", Duration::from_secs(10)).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), ("LOGIN_DENIED", "access_denied"));
        client.join().unwrap();
    }
}
