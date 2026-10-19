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
fn curl_catalog_skips_current_and_force_reruns() {
    let env = Env::new();
    let ws = Workspace::open(env.p("pub-cat"));
    rt().block_on(ws.init(365)).unwrap();
    // The installer drops a `scout` that reports 1.2.0 and logs each run.
    let runs = env.p("runs");
    let bin = env.home().join("bin");
    let script = env.p("install.sh");
    let write_scout = |v: &str| format!("mkdir -p {b} && printf '#!/bin/sh\\necho scout {v}\\n' > {b}/scout && chmod +x {b}/scout", b = bin.display());
    std::fs::write(&script, format!("#!/bin/sh\necho run >> {}\n{}\n", runs.display(), write_scout("1.2.0"))).unwrap();
    let entry: w3bs_schema::manifest::CatalogEntry = serde_json::from_value(serde_json::json!({
        "id": "tools.example/scout", "name": "scout", "description": "scripted", "commands": ["scout"],
        "install": {"url": "https://example.invalid/install.sh", "shell": "sh"}, "latest_version": "1.2.0"
    }))
    .unwrap();
    ws.set_catalog(vec![entry]).unwrap();
    let p = ws.staging().join("catalog/index.json");
    let s = std::fs::read_to_string(&p).unwrap().replace("https://example.invalid/install.sh", &format!("file://{}", script.display()));
    std::fs::write(&p, s).unwrap();
    publish(&ws);
    add_repo(&env, "cat", &ws);
    let count = || std::fs::read_to_string(&runs).map(|s| s.lines().count()).unwrap_or(0);

    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "installed".into(), 1), "{v}");
    assert_eq!(v["packages"][0]["to"], "1.2.0", "{v}");
    // Current: neither update nor install re-runs the installer.
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "current".into(), 1), "{v}");
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "current".into(), 1), "{v}");
    let (_, v, _) = env.w3bs(&["outdated"]);
    assert_eq!(status(&v, "tools.example/scout"), "current", "{v}");
    // --force re-runs it either way; the same version afterwards still reads as current.
    let (code, v, _) = env.w3bs(&["update", "--force", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "current".into(), 2), "{v}");
    assert_eq!(v["packages"][0]["reason"], "re-ran the publisher's curl installer; no newer version", "{v}");
    let (code, v, _) = env.w3bs(&["install", "scout", "--force", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "current".into(), 3), "{v}");
    // An older command on disk is updated.
    std::process::Command::new("sh").arg("-c").arg(write_scout("1.1.0")).status().unwrap();
    let (_, v, _) = env.w3bs(&["outdated"]);
    assert_eq!((v["packages"][0]["from"].as_str(), v["packages"][0]["to"].as_str()), (Some("1.1.0"), Some("1.2.0")), "{v}");
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "updated".into(), 4), "{v}");
    assert_eq!(env.run_cmd("scout"), "scout 1.2.0");
    // Already on the machine but unknown to W3BS (an OS image): adopted, installer not run.
    let (code, _, _) = env.w3bs(&["remove", "scout", "--yes"]);
    assert_eq!(code, 0);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/scout"), count()), (0, "current".into(), 4), "{v}");
    let (_, l, _) = env.w3bs(&["list"]);
    assert_eq!(l["data"]["packages"][0]["id"], "tools.example/scout", "{l}");
}

#[test]
fn force_reinstalls_native_release_and_keeps_rollback() {
    let env = Env::new();
    let a = publisher(&env, "tools.example", "atlas", &["1.0.0", "1.0.1"]);
    add_repo(&env, "tools", &a);
    let (code, _, _) = env.w3bs(&["install", "atlas", "--version", "1.0.0", "--yes"]);
    assert_eq!(code, 0);
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/atlas")), (0, "updated".into()), "{v}");
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!((code, status(&v, "tools.example/atlas")), (0, "current".into()), "{v}");
    for args in [&["update", "--force", "--yes"][..], &["install", "atlas", "--force", "--yes"][..]] {
        let (code, v, _) = env.w3bs(args);
        assert_eq!((code, status(&v, "tools.example/atlas")), (0, "installed".into()), "{args:?}: {v}");
        assert_eq!(v["packages"][0]["reason"], "reinstalled (--force)", "{v}");
        assert_eq!(env.run_cmd("atlas"), "atlas 1.0.1");
    }
    let (code, _, _) = env.w3bs(&["doctor"]);
    assert_eq!(code, 0);
    let (code, v, _) = env.w3bs(&["rollback", "atlas", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(env.run_cmd("atlas"), "atlas 1.0.0");
}

#[test]
fn packages_lists_the_signed_catalog() {
    let env = Env::new();
    let ws = Workspace::open(env.p("pub-cat"));
    rt().block_on(ws.init(365)).unwrap();
    let entry = |id: &str, agentic: bool, vendor: &str| -> w3bs_schema::manifest::CatalogEntry {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id.split('/').nth(1).unwrap(), "description": "d", "vendor": vendor, "agentic": agentic,
            "install": {"url": format!("https://{}/install.sh", id.split('/').next().unwrap()), "shell": "sh"}
        }))
        .unwrap()
    };
    ws.set_catalog(vec![entry("moshcode.sh/moshcode", true, "Profullstack"), entry("ollama.com/ollama", false, "Ollama")]).unwrap();
    publish(&ws);
    add_repo(&env, "w3bs", &ws);
    let (code, v, _) = env.w3bs(&["packages"]);
    assert_eq!((code, v["data"]["count"].as_u64()), (0, Some(2)), "{v}");
    let (_, v, _) = env.w3bs(&["packages", "--agentic"]);
    assert_eq!(v["data"]["packages"][0]["id"], "moshcode.sh/moshcode");
    assert_eq!(v["data"]["packages"][0]["install"], "curl -fsSL https://moshcoding.sh/install.sh | sh".replace("moshcoding.sh", "moshcode.sh"));
    let (_, v, _) = env.w3bs(&["pkg", "catalog", "--ours"]);
    assert_eq!(v["data"]["count"], 1);
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

// ------------------------------------------------------------------ W3BS-SIGN-1 publisher signing

/// A fresh ed25519 key made by the real ssh-keygen; returns the private key path.
fn ssh_key(env: &Env, name: &str) -> PathBuf {
    let k = env.p(&format!("keys/{name}"));
    std::fs::create_dir_all(k.parent().unwrap()).unwrap();
    let ok = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"]).arg(&k).status().unwrap();
    assert!(ok.success(), "ssh-keygen -t ed25519");
    k
}

fn pub_line(key: &Path) -> String {
    std::fs::read_to_string(format!("{}.pub", key.display())).unwrap().trim().to_owned()
}

fn fingerprint(key: &Path) -> String {
    w3bs_core::sshsig::PublicKey::parse_openssh(&pub_line(key)).unwrap().fingerprint()
}

/// `ssh-keygen -Y sign -n w3bs-release@w3bs.org` over the statement's JCS bytes.
fn ssh_sign_statement(env: &Env, key: &Path, statement: &Value) -> String {
    let f = env.p("statement.jcs.json");
    std::fs::write(&f, w3bs_core::release::statement_bytes(statement).unwrap()).unwrap();
    let sig = PathBuf::from(format!("{}.sig", f.display()));
    let _ = std::fs::remove_file(&sig);
    let ok = Command::new("ssh-keygen").args(["-q", "-Y", "sign", "-n", "w3bs-release@w3bs.org", "-f"]).arg(key).arg(&f).status().unwrap();
    assert!(ok.success(), "ssh-keygen -Y sign");
    std::fs::read_to_string(sig).unwrap()
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

/// The installer for `scout`: logs each run, then drops a `scout` reporting `version`.
fn scout_installer(env: &Env, version: &str) -> PathBuf {
    let script = env.p(&format!("install-{version}.sh"));
    let bin = env.home().join("bin");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho {version} >> {runs}\nmkdir -p {b} && printf '#!/bin/sh\\necho scout {version}\\n' > {b}/scout && chmod +x {b}/scout\n", runs = env.p("runs").display(), b = bin.display()),
    )
    .unwrap();
    script
}

/// A catalog entry for `tools.example/scout` at `version`, installed by `script`. With a
/// key, the entry carries the publisher's signature (over the installer's current bytes).
fn scout_entry(env: &Env, version: &str, script: &Path, key: Option<&Path>) -> Value {
    let url = format!("file://{}", script.display());
    let mut entry = serde_json::json!({
        "id": "tools.example/scout", "name": "scout", "description": "signed scout", "commands": ["scout"],
        "install": {"url": url, "shell": "sh"}, "latest_version": version
    });
    if let Some(key) = key {
        let statement = serde_json::json!({
            "type": "w3bs-release/1", "package": "tools.example/scout", "version": version, "name": "scout",
            "description": "signed scout", "commands": ["scout"], "install": format!("curl -fsSL {url} | sh"),
            "installer": {"url": url, "sha256": sha256_hex(&std::fs::read(script).unwrap())},
            "created_at": "2026-10-19T20:00:00Z"
        });
        let signature = ssh_sign_statement(env, key, &statement);
        entry["signed"] = serde_json::json!({ "statement": statement, "signature": signature, "public_key": pub_line(key), "fingerprint": fingerprint(key) });
    }
    entry
}

/// Publish exactly these catalog entries (written as-is: signed statements must not be
/// rewritten after signing).
fn set_raw_catalog(ws: &Workspace, entries: Vec<Value>) {
    ws.set_catalog(vec![]).unwrap();
    let p = ws.staging().join("catalog/index.json");
    std::fs::write(&p, serde_json::to_vec_pretty(&serde_json::json!({ "schema_version": "1.0", "entries": entries })).unwrap()).unwrap();
    publish(ws);
}

fn catalog_repo(env: &Env) -> Workspace {
    let ws = Workspace::open(env.p("pub-cat"));
    rt().block_on(ws.init(365)).unwrap();
    ws
}

fn runs(env: &Env) -> Vec<String> {
    std::fs::read_to_string(env.p("runs")).map(|s| s.lines().map(str::to_owned).collect()).unwrap_or_default()
}

fn outcome<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["packages"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap_or_else(|| panic!("no outcome for {id}: {v}"))
}

#[test]
fn signed_release_installs_and_is_recorded() {
    let env = Env::new();
    let ws = catalog_repo(&env);
    let key = ssh_key(&env, "publisher");
    let script = scout_installer(&env, "1.0.0");
    set_raw_catalog(&ws, vec![scout_entry(&env, "1.0.0", &script, Some(&key))]);
    add_repo(&env, "cat", &ws);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(runs(&env), vec!["1.0.0"]);
    let fp = fingerprint(&key);
    assert_eq!(outcome(&v, "tools.example/scout")["signature"], format!("signed by {fp}"), "{v}");
    let (_, info, _) = env.w3bs(&["info", "scout"]);
    assert_eq!((info["data"]["signature"]["status"].as_str(), info["data"]["signature"]["fingerprint"].as_str()), (Some("signed"), Some(fp.as_str())), "{info}");
    assert_eq!(info["data"]["signature"]["installer_sha256"], sha256_hex(&std::fs::read(&script).unwrap()));
}

#[test]
fn signed_release_with_tampered_installer_never_runs() {
    let env = Env::new();
    let ws = catalog_repo(&env);
    let key = ssh_key(&env, "publisher");
    let script = scout_installer(&env, "1.0.0");
    let entry = scout_entry(&env, "1.0.0", &script, Some(&key));
    // The script changes after it was signed (a compromised host, a swapped file).
    std::fs::write(&script, format!("#!/bin/sh\necho evil >> {}\n", env.p("runs").display())).unwrap();
    set_raw_catalog(&ws, vec![entry]);
    add_repo(&env, "cat", &ws);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!(code, 1, "{v}");
    assert_eq!(outcome(&v, "tools.example/scout")["error"]["code"], "INSTALLER_DIGEST_MISMATCH", "{v}");
    assert!(runs(&env).is_empty(), "the tampered installer ran");
    let (_, l, _) = env.w3bs(&["list"]);
    assert!(l["data"]["packages"].as_array().is_none_or(|a| a.is_empty()), "{l}");
}

#[test]
fn signed_release_by_wrong_key_refused() {
    let env = Env::new();
    let ws = catalog_repo(&env);
    let key = ssh_key(&env, "publisher");
    let other = ssh_key(&env, "attacker");
    let script = scout_installer(&env, "1.0.0");
    // Signed by `other`, but listed under the publisher's key.
    let mut entry = scout_entry(&env, "1.0.0", &script, Some(&other));
    entry["signed"]["public_key"] = pub_line(&key).into();
    entry["signed"]["fingerprint"] = fingerprint(&key).into();
    // And a statement whose signature was made under another namespace (a git signature).
    let mut git = scout_entry(&env, "1.0.0", &script, Some(&key));
    let f = env.p("git.json");
    std::fs::write(&f, w3bs_core::release::statement_bytes(&git["signed"]["statement"]).unwrap()).unwrap();
    assert!(Command::new("ssh-keygen").args(["-q", "-Y", "sign", "-n", "git", "-f"]).arg(&key).arg(&f).status().unwrap().success());
    git["signed"]["signature"] = std::fs::read_to_string(env.p("git.json.sig")).unwrap().into();
    for (name, entry) in [("wrong key", entry), ("wrong namespace", git)] {
        set_raw_catalog(&ws, vec![entry]);
        if name == "wrong key" {
            add_repo(&env, "cat", &ws);
        }
        let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
        assert_eq!(code, 1, "{name}: {v}");
        // Refused while planning, so the outcome carries the name as given.
        assert_eq!(outcome(&v, "scout")["error"]["code"], "SIGNATURE_INVALID", "{name}: {v}");
        assert!(runs(&env).is_empty(), "{name}: installer ran");
    }
}

#[test]
fn publisher_key_change_needs_accept_new_key() {
    let env = Env::new();
    let ws = catalog_repo(&env);
    let key = ssh_key(&env, "publisher");
    let rotated = ssh_key(&env, "rotated");
    let v1 = scout_installer(&env, "1.0.0");
    set_raw_catalog(&ws, vec![scout_entry(&env, "1.0.0", &v1, Some(&key))]);
    add_repo(&env, "cat", &ws);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!(code, 0, "{v}");

    // 1.0.1 arrives signed by a key this machine has not pinned for tools.example.
    let v2 = scout_installer(&env, "1.0.1");
    set_raw_catalog(&ws, vec![scout_entry(&env, "1.0.1", &v2, Some(&rotated))]);
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!(code, 1, "{v}");
    let o = outcome(&v, "tools.example/scout");
    assert_eq!((o["status"].as_str(), o["error"]["code"].as_str()), (Some("blocked"), Some("PUBLISHER_KEY_CHANGED")), "{v}");
    assert_eq!(runs(&env), vec!["1.0.0"], "installer ran under an unpinned key");
    // Pins outlive `remove`: reinstalling does not quietly trust the new key either.
    let (code, _, _) = env.w3bs(&["remove", "scout", "--yes"]);
    assert_eq!(code, 0);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!((code, v["error"]["code"].as_str()), (3, Some("PUBLISHER_KEY_CHANGED")), "{v}");
    assert_eq!(runs(&env), vec!["1.0.0"]);
    // Explicit acceptance pins the new key alongside the old one.
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes", "--accept-new-key"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(runs(&env), vec!["1.0.0", "1.0.1"]);
    assert_eq!(outcome(&v, "tools.example/scout")["signature"], format!("signed by {}", fingerprint(&rotated)));
    let (code, v, _) = env.w3bs(&["install", "scout", "--force", "--yes"]);
    assert_eq!(code, 0, "now pinned: {v}");
}

#[test]
fn stripped_signature_on_a_pinned_namespace_is_refused() {
    let env = Env::new();
    let ws = catalog_repo(&env);
    let key = ssh_key(&env, "publisher");
    let v1 = scout_installer(&env, "1.0.0");
    set_raw_catalog(&ws, vec![scout_entry(&env, "1.0.0", &v1, Some(&key))]);
    add_repo(&env, "cat", &ws);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!(code, 0, "{v}");

    // 1.0.1 arrives with its signature removed: a downgrade to "unsigned" is not allowed
    // once this machine has pinned the namespace's key.
    let v2 = scout_installer(&env, "1.0.1");
    set_raw_catalog(&ws, vec![scout_entry(&env, "1.0.1", &v2, None)]);
    let (code, v, _) = env.w3bs(&["update", "--yes"]);
    assert_eq!(code, 1, "{v}");
    let o = outcome(&v, "tools.example/scout");
    assert_eq!((o["status"].as_str(), o["error"]["code"].as_str()), (Some("blocked"), Some("PUBLISHER_SIGNATURE_MISSING")), "{v}");
    assert_eq!(runs(&env), vec!["1.0.0"], "the unsigned installer ran");
    let (code, v, _) = env.w3bs(&["update", "--yes", "--accept-new-key"]);
    assert_eq!(code, 0, "explicit acceptance: {v}");
    assert_eq!(runs(&env), vec!["1.0.0", "1.0.1"]);
}

#[test]
fn unsigned_entries_install_as_before_and_say_so() {
    let env = Env::new();
    let ws = catalog_repo(&env);
    let script = scout_installer(&env, "1.0.0");
    set_raw_catalog(&ws, vec![scout_entry(&env, "1.0.0", &script, None)]);
    add_repo(&env, "cat", &ws);
    let (code, v, _) = env.w3bs(&["install", "scout", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(outcome(&v, "tools.example/scout")["signature"], "unsigned");
    let (_, info, _) = env.w3bs(&["info", "scout"]);
    assert_eq!(info["data"]["signature"], "unsigned", "{info}");
}

// --------------------------------------------------- `w3bs publish` against a stand-in API

/// A minimal HTTP/1.1 server for the publish API contract; records every request.
struct FakeApi {
    origin: String,
    seen: std::sync::Arc<std::sync::Mutex<Vec<(String, String, Value)>>>,
}

impl FakeApi {
    fn start(installer: Vec<u8>) -> Self {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let log = seen.clone();
        let base = origin.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_owned(), parts.next().unwrap_or("").to_owned());
                let mut len = 0usize;
                let mut auth = String::new();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        auth = h.trim().to_owned();
                    }
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                log.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
                let api = path.starts_with("/api/publish");
                let (status, ctype, out): (u16, &str, Vec<u8>) = if api && auth != "authorization: Bearer test-token" && auth != "Authorization: Bearer test-token" {
                    (401, "application/json", br#"{"error":{"code":"UNAUTHORIZED","message":"sign in"}}"#.to_vec())
                } else {
                    match (method.as_str(), path.as_str()) {
                        ("GET", "/install.sh") => (200, "text/plain", installer.clone()),
                        ("GET", "/api/publish/keys/challenge") => (200, "application/json", br#"{"challenge":"c0ffee","message":"w3bs-key:alice:c0ffee","expires_at":"2099-01-01T00:00:00Z"}"#.to_vec()),
                        ("POST", "/api/publish/keys") => {
                            let fp = w3bs_core::sshsig::PublicKey::parse_openssh(body["public_key"].as_str().unwrap_or_default()).map(|k| k.fingerprint()).unwrap_or_default();
                            let ns = body["namespace"].as_str().unwrap_or_default();
                            let v = serde_json::json!({ "fingerprint": fp, "namespace": ns, "domain_verified": false,
                                "proof": { "well_known": format!("https://{ns}/.well-known/w3bs-publisher.txt"), "dns": format!("_w3bs.{ns}"), "line": format!("w3bs-publisher={fp}") } });
                            (200, "application/json", v.to_string().into_bytes())
                        }
                        ("POST", "/api/publish/releases") => {
                            let st = &body["statement"];
                            let v = serde_json::json!({ "package": st["package"], "version": st["version"], "fingerprint": "x", "url": format!("{base}/packages/{}", st["package"].as_str().unwrap_or_default()) });
                            (200, "application/json", v.to_string().into_bytes())
                        }
                        _ => (404, "application/json", br#"{"error":{"code":"NOT_FOUND","message":"no such thing"}}"#.to_vec()),
                    }
                };
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
                let _ = stream.write_all(&out);
            }
        });
        Self { origin, seen }
    }

    fn requests(&self, method: &str, path: &str) -> Vec<Value> {
        self.seen.lock().unwrap().iter().filter(|(m, p, _)| m == method && p == path).map(|(_, _, b)| b.clone()).collect()
    }
}

/// Sign in to the fake API the way `w3bs login` would have left it.
fn fake_login(env: &Env, origin: &str) {
    let dir = env.p("config/w3bs");
    std::fs::create_dir_all(&dir).unwrap();
    let creds = serde_json::json!({ origin: { "access_token": "test-token", "refresh_token": "r", "expires_at": 4_000_000_000u64, "username": "alice" } });
    std::fs::write(dir.join("auth.json"), creds.to_string()).unwrap();
}

#[test]
fn publish_key_add_and_release_against_the_api_contract() {
    let env = Env::new();
    let installer = b"#!/bin/sh\necho hi\n".to_vec();
    let api = FakeApi::start(installer.clone());
    fake_login(&env, &api.origin);
    let key = ssh_key(&env, "publisher");
    let (config, nohome) = (env.p("config"), env.p("nohome"));
    let envs = [("XDG_CONFIG_HOME", config.to_str().unwrap()), ("HOME", nohome.to_str().unwrap())];
    let fp = fingerprint(&key);

    let (code, v, err) = env.w3bs_env(&["publish", "key", "add", "tools.example", "--key", key.to_str().unwrap(), "--api", &api.origin], &envs);
    assert_eq!(code, 0, "{v} {err}");
    assert_eq!(v["data"]["fingerprint"], fp);
    assert_eq!(v["data"]["proof"]["line"], format!("w3bs-publisher={fp}"));
    assert_eq!(v["data"]["proof"]["dns"], "_w3bs.tools.example");
    let posted = &api.requests("POST", "/api/publish/keys")[0];
    assert_eq!((posted["namespace"].as_str(), posted["challenge"].as_str(), posted["public_key"].as_str()), (Some("tools.example"), Some("c0ffee"), Some(pub_line(&key).as_str())));
    // The registration signature is over exactly the challenge message, in the key namespace.
    let ok = w3bs_core::sshsig::verify(posted["signature"].as_str().unwrap(), "w3bs-key@w3bs.org", b"w3bs-key:alice:c0ffee", None).unwrap();
    assert_eq!(ok.fingerprint, fp);

    let install = format!("curl -fsSL {}/install.sh | sh", api.origin);
    let args = |dry: bool| {
        let mut a = vec!["publish", "release", "--package", "tools.example/scout", "--version", "1.2.0", "--install", &install, "--description", "a scout", "--command", "scout", "--license", "MIT", "--key", key.to_str().unwrap(), "--api", &api.origin];
        if dry {
            a.push("--dry-run");
        }
        a.into_iter().map(str::to_owned).collect::<Vec<_>>()
    };
    let dry = args(true);
    let (code, v, err) = env.w3bs_env(&dry.iter().map(String::as_str).collect::<Vec<_>>(), &envs);
    assert_eq!(code, 0, "{v} {err}");
    assert!(api.requests("POST", "/api/publish/releases").is_empty(), "dry run submitted");
    let st = &v["data"]["statement"];
    assert_eq!((st["type"].as_str(), st["package"].as_str(), st["version"].as_str()), (Some("w3bs-release/1"), Some("tools.example/scout"), Some("1.2.0")));
    assert_eq!(st["installer"]["sha256"], sha256_hex(&installer));
    w3bs_core::release::check_statement_with(st, true).unwrap();

    let real = args(false);
    let (code, v, err) = env.w3bs_env(&real.iter().map(String::as_str).collect::<Vec<_>>(), &envs);
    assert_eq!(code, 0, "{v} {err}");
    assert_eq!(v["data"]["url"], format!("{}/packages/tools.example/scout", api.origin));
    let sub = &api.requests("POST", "/api/publish/releases")[0];
    // Anyone can check the submitted release offline with ssh-keygen (W3BS-SIGN-1 §4).
    std::fs::write(env.p("release.jcs.json"), w3bs_core::release::statement_bytes(&sub["statement"]).unwrap()).unwrap();
    std::fs::write(env.p("release.sig"), sub["signature"].as_str().unwrap()).unwrap();
    std::fs::write(env.p("allowed_signers"), format!("tools.example namespaces=\"w3bs-release@w3bs.org\" {}\n", pub_line(&key))).unwrap();
    let out = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-I", "tools.example", "-n", "w3bs-release@w3bs.org", "-f"])
        .arg(env.p("allowed_signers"))
        .arg("-s")
        .arg(env.p("release.sig"))
        .stdin(std::fs::File::open(env.p("release.jcs.json")).unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "ssh-keygen -Y verify: {}", String::from_utf8_lossy(&out.stderr));

    // Without a session the API is not called with stale credentials.
    std::fs::remove_file(env.p("config/w3bs/auth.json")).unwrap();
    let (code, v, _) = env.w3bs_env(&["publish", "key", "ls", "--api", &api.origin], &envs);
    assert_eq!((code, v["error"]["code"].as_str()), (1, Some("NOT_SIGNED_IN")), "{v}");
}
