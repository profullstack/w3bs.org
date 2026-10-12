//! End-to-end acceptance tests: real `w3bs` binary, real signed TUF repositories on disk.
//! Each test gets its own W3BS_HOME, so nothing touches the user's store.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use w3bs_publish::{Expiry, ReleaseSpec, Workspace};
use w3bs_schema::manifest::ArtifactFormat;

const PLATFORM: &str = "x86_64-unknown-linux-gnu";

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self { dir: tempfile::tempdir().unwrap() }
    }
    fn p(&self, s: &str) -> PathBuf {
        self.dir.path().join(s)
    }
    fn home(&self) -> PathBuf {
        self.p("home")
    }
    fn w3bs(&self, args: &[&str]) -> (i32, Value, String) {
        self.w3bs_env(args, &[])
    }
    fn w3bs_env(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, Value, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_w3bs"));
        c.args(args).arg("--json").env("W3BS_HOME", self.home()).env("W3BS_DEV_INSECURE_TRANSPORT", "1").env("W3BS_LOCK_TIMEOUT_SECS", "1");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
        let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {stdout}"));
        (o.status.code().unwrap_or(-1), v, String::from_utf8_lossy(&o.stderr).into_owned())
    }
    fn run_cmd(&self, name: &str) -> String {
        let o = Command::new(self.home().join("bin").join(name)).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_owned()
    }
}

fn fixture(env: &Env, name: &str, version: &str) -> PathBuf {
    let d = env.p(&format!("build/{name}-{version}"));
    std::fs::create_dir_all(d.join("bin")).unwrap();
    let bin = d.join("bin").join(name);
    std::fs::write(&bin, format!("#!/bin/sh\necho \"{name} {version}\"\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = env.p(&format!("build/{name}-{version}.tar.gz"));
    let f = std::fs::File::create(&out).unwrap();
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(f, flate2::Compression::fast()));
    b.append_dir_all("bin", d.join("bin")).unwrap();
    b.into_inner().unwrap().finish().unwrap();
    out
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// A publisher workspace with `name` published at the given versions.
fn publisher(env: &Env, host: &str, name: &str, versions: &[&str]) -> Workspace {
    let ws = Workspace::open(env.p(&format!("pub-{name}")));
    rt().block_on(ws.init(365)).unwrap();
    for v in versions {
        release(env, &ws, host, name, v);
    }
    rt().block_on(ws.build(Expiry::default())).unwrap();
    ws
}

fn release(env: &Env, ws: &Workspace, host: &str, name: &str, v: &str) {
    let art = fixture(env, name, v);
    ws.add_release(ReleaseSpec {
        id: format!("{host}/{name}").parse().unwrap(),
        version: v.parse().unwrap(),
        channel: "stable".into(),
        description: format!("{name} fixture"),
        license: "MIT".into(),
        format: ArtifactFormat::TarGz,
        artifacts: vec![(PLATFORM.into(), art)],
        commands: [(name.to_string(), format!("bin/{name}"))].into(),
        manager_requirement: None,
    })
    .unwrap();
}

fn publish(ws: &Workspace) {
    rt().block_on(ws.build(Expiry::default())).unwrap();
}

fn url(ws: &Workspace) -> String {
    format!("file://{}/", ws.repo().display())
}

fn add_repo(env: &Env, alias: &str, ws: &Workspace) {
    let fp = ws.fingerprint().unwrap();
    let (code, v, err) = env.w3bs(&["repo", "add", alias, &url(ws), "--root-fingerprint", &fp, "--yes"]);
    assert_eq!(code, 0, "{v} {err}");
}

fn status(v: &Value, id: &str) -> String {
    v["packages"].as_array().unwrap().iter().find(|p| p["id"] == id).map(|p| p["status"].as_str().unwrap().to_owned()).unwrap_or_default()
}

#[test]
fn at01_03_install_update_all_two_publishers() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    let b = publisher(&env, "labs.example", "beacon", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    add_repo(&env, "labs", &b);
    let (code, v, _) = env.w3bs(&["install", "atlas", "labs.example/beacon", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(env.run_cmd("atlas"), "atlas 1.0.0");
    assert_eq!(env.run_cmd("beacon"), "beacon 1.0.0");
    release(&env, &a, "tools.example", "atlas", "1.0.1");
    publish(&a);
    release(&env, &b, "labs.example", "beacon", "1.0.1");
    publish(&b);
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(status(&v, "tools.example/atlas"), "updated");
    assert_eq!(status(&v, "labs.example/beacon"), "updated");
    assert_eq!(env.run_cmd("atlas"), "atlas 1.0.1");
    let (_, info, _) = env.w3bs(&["info", "atlas"]);
    assert_eq!(info["data"]["previous_version"], "1.0.0");
}

#[test]
fn at15_headless_unknown_root_needs_trust() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    let (code, v, _) = env.w3bs(&["repo", "add", "tools", &url(&a), "--yes"]);
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["error"]["code"], "TRUST_DECISION_REQUIRED");
    let (code, v, _) = env.w3bs(&["repo", "add", "tools", &url(&a), "--root-fingerprint", "sha256:00", "--yes"]);
    assert_eq!(code, 4, "{v}");
}

#[test]
fn at37_mutation_without_yes_returns_3() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    let (code, v, _) = env.w3bs(&["install", "atlas"]);
    assert_eq!(code, 3, "{v}");
    assert!(!env.home().join("bin/atlas").exists());
}

#[test]
fn at07_tampered_artifact_rejected() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    let target = a.repo().join("targets/artifacts/atlas/1.0.0/atlas-1.0.0.tar.gz");
    let mut bytes = std::fs::read(&target).unwrap();
    let n = bytes.len();
    bytes[n - 5] ^= 0xff;
    std::fs::write(&target, bytes).unwrap();
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!(code, 1, "{v}");
    assert!(!env.home().join("bin/atlas").exists());
    let (_, l, _) = env.w3bs(&["list"]);
    assert_eq!(l["data"]["packages"].as_array().unwrap().len(), 0);
}

#[test]
fn at07_tampered_manifest_rejected() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    let m = a.repo().join("targets/manifests/atlas/1.0.0.json");
    let s = std::fs::read_to_string(&m).unwrap().replace("atlas fixture", "atlas fixturf");
    std::fs::write(&m, s).unwrap();
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!(code, 1, "{v}");
}

#[test]
fn at09_crash_at_every_checkpoint_recovers() {
    for cp in ["journal", "staged", "activated", "linked", "committed"] {
        let env = Env::new();
        let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
        add_repo(&env, "tools", &a);
        let (code, _, _) = env.w3bs(&["install", "atlas", "--yes"]);
        assert_eq!(code, 0);
        release(&env, &a, "tools.example", "atlas", "1.0.1");
        publish(&a);
        let o = Command::new(env!("CARGO_BIN_EXE_w3bs"))
            .args(["update", "--yes", "--json"])
            .env("W3BS_HOME", env.home())
            .env("W3BS_DEV_INSECURE_TRANSPORT", "1")
            .env("W3BS_TEST_CRASH_AT", cp)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(99), "crash hook at {cp}");
        // Read-only commands report, never recover.
        let (_, l, _) = env.w3bs(&["list"]);
        if cp != "committed" {
            assert!(l["warnings"].to_string().contains("repair"), "{cp}: {l}");
        }
        let (code, _, _) = env.w3bs(&["repair"]);
        assert_eq!(code, 0);
        let (_, info, _) = env.w3bs(&["info", "atlas"]);
        let v = info["data"]["version"].as_str().unwrap().to_owned();
        assert_eq!(env.run_cmd("atlas"), format!("atlas {v}"), "{cp}: binary must match committed state");
        let (code, d, _) = env.w3bs(&["doctor"]);
        assert_eq!(code, 0, "{cp}: {d}");
        if cp == "committed" {
            assert_eq!(v, "1.0.1");
        } else {
            assert_eq!(v, "1.0.0", "{cp}");
        }
    }
}

#[test]
fn at13_command_collision_refused() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    std::fs::create_dir_all(env.home().join("bin")).unwrap();
    std::fs::write(env.home().join("bin/atlas"), "#!/bin/sh\necho mine\n").unwrap();
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!(code, 1);
    assert!(v.to_string().contains("COMMAND_CONFLICT"), "{v}");
    assert_eq!(std::fs::read_to_string(env.home().join("bin/atlas")).unwrap(), "#!/bin/sh\necho mine\n");
}

#[test]
fn at06_major_held_and_at32_pin_binding() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    env.w3bs(&["install", "atlas", "--yes"]);
    release(&env, &a, "tools.example", "atlas", "2.0.0");
    publish(&a);
    let (_, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!(status(&v, "tools.example/atlas"), "held");
    env.w3bs(&["pin", "atlas", "--yes"]);
    let (_, v, _) = env.w3bs(&["update", "--yes", "--allow-major"]);
    assert_eq!(status(&v, "tools.example/atlas"), "pinned");
    env.w3bs(&["unpin", "atlas", "--yes"]);
    let (_, v, _) = env.w3bs(&["update", "--yes", "--allow-major"]);
    assert_eq!(status(&v, "tools.example/atlas"), "updated");
    assert_eq!(env.run_cmd("atlas"), "atlas 2.0.0");
}

#[test]
fn at33_reinstall_is_idempotent_and_version_change_conflicts() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0", "1.0.1"]);
    add_repo(&env, "tools", &a);
    let (code, _, _) = env.w3bs(&["install", "atlas", "--version", "1.0.0", "--yes"]);
    assert_eq!(code, 0);
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/atlas")), (0, "current".into()));
    let (code, v, _) = env.w3bs(&["install", "atlas", "--version", "1.0.1", "--yes"]);
    assert_eq!(code, 1, "{v}");
    assert_eq!(env.run_cmd("atlas"), "atlas 1.0.0");
}

#[test]
fn at36_argument_errors_exit_2_without_mutation() {
    let env = Env::new();
    for args in [vec!["install"], vec!["remove"], vec!["install", "Bad/Name"], vec!["update", "--all", "x.example/y"]] {
        let o = Command::new(env!("CARGO_BIN_EXE_w3bs")).args(&args).env("W3BS_HOME", env.home()).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
    assert!(!env.home().join("data/state.sqlite3").exists());
    let o = Command::new(env!("CARGO_BIN_EXE_w3bs")).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("w3bs "));
}

#[test]
fn at24_immutable_version_violation_detected() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    env.w3bs(&["install", "atlas", "--yes"]);
    env.w3bs(&["remove", "atlas", "--yes"]);
    // The publisher rewrites 1.0.0 behind the tooling's back and re-signs.
    let other = fixture(&env, "atlas", "1.0.0-evil");
    std::fs::copy(&other, a.staging().join("artifacts/atlas/1.0.0/atlas-1.0.0.tar.gz")).unwrap();
    let mp = a.staging().join("manifests/atlas/1.0.0.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&mp).unwrap()).unwrap();
    let bytes = std::fs::read(&other).unwrap();
    use sha2::Digest;
    m["artifacts"][0]["sha256"] = hex::encode(sha2::Sha256::digest(&bytes)).into();
    m["artifacts"][0]["length"] = bytes.len().into();
    std::fs::write(&mp, serde_json::to_vec(&m).unwrap()).unwrap();
    publish(&a);
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!(code, 1);
    assert!(v.to_string().contains("IMMUTABILITY_VIOLATION"), "{v}");
}

#[test]
fn revoked_release_blocked() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0", "1.0.1"]);
    a.revoke(&"tools.example/atlas".parse().unwrap(), &"1.0.1".parse().unwrap(), "bad build").unwrap();
    publish(&a);
    add_repo(&env, "tools", &a);
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!(code, 1, "{v}");
    assert!(v.to_string().contains("REVOKED"), "{v}");
    let (code, _, _) = env.w3bs(&["install", "atlas", "--version", "1.0.0", "--yes"]);
    assert_eq!(code, 0);
}

#[test]
fn at38_repo_remove_refused_while_in_use() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    env.w3bs(&["install", "atlas", "--yes"]);
    let (code, _, _) = env.w3bs(&["repo", "remove", "tools", "--yes"]);
    assert_eq!(code, 1);
    env.w3bs(&["remove", "atlas", "--yes"]);
    let (code, _, _) = env.w3bs(&["repo", "remove", "tools", "--yes"]);
    assert_eq!(code, 0);
}

#[test]
fn at22_dry_run_changes_nothing() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    let (code, v, _) = env.w3bs(&["install", "atlas", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(status(&v, "tools.example/atlas"), "planned");
    assert!(!env.home().join("bin/atlas").exists());
    assert!(!env.home().join("cache/blobs").exists());
}

#[test]
fn curl_catalog_entry_runs_publisher_installer() {
    let env = Env::new();
    let ws = Workspace::open(env.p("pub-cat"));
    rt().block_on(ws.init(365)).unwrap();
    // The publisher's own installer, served over file:// in dev mode.
    let marker = env.p("ran");
    let script = env.p("install.sh");
    std::fs::write(&script, format!("#!/bin/sh\necho installed > {}\n", marker.display())).unwrap();
    let entry: w3bs_schema::manifest::CatalogEntry = serde_json::from_value(serde_json::json!({
        "id": "tools.example/scout", "name": "scout", "description": "scripted", "commands": ["scout"],
        "install": {"url": "https://example.invalid/install.sh", "shell": "sh"}, "agentic": true
    }))
    .unwrap();
    ws.set_catalog(vec![entry]).unwrap();
    // Point the signed entry at the local script (dev transport only).
    let p = ws.staging().join("catalog/index.json");
    let s = std::fs::read_to_string(&p).unwrap().replace("https://example.invalid/install.sh", &format!("file://{}", script.display()));
    std::fs::write(&p, s).unwrap();
    publish(&ws);
    add_repo(&env, "cat", &ws);
    let (_, v, _) = env.w3bs(&["search", "scout"]);
    assert_eq!(v["data"]["results"][0]["kind"], "curl");
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert!(marker.exists(), "installer ran");
    let (_, l, _) = env.w3bs(&["list"]);
    assert_eq!(l["data"]["packages"][0]["kind"], "curl");
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout")), (0, "updated".into()), "{v}");
    let (code, _, _) = env.w3bs(&["remove", "scout", "--yes"]);
    assert_eq!(code, 0);
}

#[test]
fn at14_concurrent_mutation_is_locked() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0"]);
    add_repo(&env, "tools", &a);
    let lock = env.home().join("state/lock");
    let f = std::fs::OpenOptions::new().write(true).open(&lock).unwrap();
    use fs4::fs_std::FileExt;
    FileExt::lock_exclusive(&f).unwrap();
    let (code, v, _) = env.w3bs(&["install", "atlas", "--yes"]);
    assert_eq!(code, 5, "{v}");
    FileExt::unlock(&f).unwrap();
    let _ = Path::new("");
}
