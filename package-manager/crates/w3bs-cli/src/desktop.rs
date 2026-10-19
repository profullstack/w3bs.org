//! `w3bs fleet desktop`: a headless Kubuntu desktop in Docker, for AI agents to work on and for
//! the fleet owner to watch on w3bs.org/fleet.
//!
//! `start` builds the image from the Dockerfile embedded here (desktop/packaging/kubuntu), runs
//! it as this user with `--restart unless-stopped`, and turns screen sharing on for this
//! machine's fleet agent through the container's VNC Unix socket. Inside: KWin on the kernel's
//! vkms virtual display (OpenGL on the CPU, no GPU needed), Plasma, Konsole, TigerVNC's
//! w0vncserver and the Agentic Desktop Layer (MCP on 127.0.0.1:7802). See
//! w3bs.org/docs/fleet-screen.md.

use crate::fleet;
use crate::screen::{self, Mode, Screen};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use w3bs_core::{Error, Result};

pub const CONTAINER: &str = "w3bs-desktop";
pub const IMAGE: &str = "w3bs-kubuntu-desktop";
/// Not 7801, which a desktop layer running natively on the same machine uses.
pub const MCP: &str = "127.0.0.1:7802";
const DOCKERFILE: &str = include_str!("../../../../desktop/packaging/kubuntu/Dockerfile");
const SESSION: &str = include_str!("../../../../desktop/packaging/kubuntu/session.sh");
const BROWSER: &str = include_str!("../../../../desktop/packaging/kubuntu/browser.sh");
const START_TIMEOUT: Duration = Duration::from_secs(120);

fn fail(code: &'static str, msg: impl Into<String>) -> Error {
    Error::package(code, msg.into())
}

/// `$XDG_DATA_HOME/w3bs/desktop`: the desktop's home (`home/`) and its VNC socket (`vnc/`).
pub fn data_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or_else(|| fail("NO_HOME", "HOME is not set"))?;
    Ok(base.join("w3bs/desktop"))
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("vnc/vnc.sock"))
}

/// The image tag: a hash of the embedded build files, so a w3bs update that changes them
/// rebuilds and an unchanged one reuses the image.
pub fn image_tag() -> String {
    let mut h = Sha256::new();
    h.update(DOCKERFILE);
    h.update([0]);
    h.update(SESSION);
    h.update([0]);
    h.update(BROWSER);
    let hex: String = h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{IMAGE}:{hex}")
}

fn docker(args: &[&str]) -> std::result::Result<String, String> {
    match Command::new("docker").args(args).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

fn require_docker() -> Result<()> {
    match docker(&["version", "--format", "{{.Server.Version}}"]) {
        Ok(_) => Ok(()),
        Err(e) if e.contains("permission denied") => Err(fail(
            "DOCKER_PERMISSION",
            format!("this user cannot use Docker; add it to the docker group and log in again: sudo usermod -aG docker {}", std::env::var("USER").unwrap_or_default()),
        )),
        Err(e) if e.contains("No such file") || e.contains("not found") => Err(fail("NO_DOCKER", "Docker is not installed (https://docs.docker.com/engine/install/)")),
        Err(e) => Err(fail("DOCKER_UNAVAILABLE", format!("Docker is not answering: {e}"))),
    }
}

/// The vkms card (`/dev/dri/cardN`): its sysfs device is named vkms (on the faux bus in recent
/// kernels, a platform device in older ones).
pub fn find_vkms(sys: &Path) -> Option<PathBuf> {
    let mut cards: Vec<_> = std::fs::read_dir(sys).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("card") && n[4..].chars().all(|c| c.is_ascii_digit()) && n.len() > 4).collect();
    cards.sort();
    cards.into_iter().find(|n| std::fs::canonicalize(sys.join(n).join("device")).ok().and_then(|p| p.file_name().map(|f| f == "vkms")).unwrap_or(false)).map(|n| PathBuf::from("/dev/dri").join(n))
}

#[cfg(unix)]
fn ids() -> (u32, u32) {
    // SAFETY: getuid/getgid have no preconditions.
    unsafe { (libc::getuid(), libc::getgid()) }
}
#[cfg(not(unix))]
fn ids() -> (u32, u32) {
    (0, 0)
}

/// Load vkms (and keep it across reboots), asking sudo for it when needed.
fn ensure_vkms(notes: &mut Vec<String>) -> Result<PathBuf> {
    let sys = Path::new("/sys/class/drm");
    if let Some(card) = find_vkms(sys) {
        return Ok(card);
    }
    let root = ids().0 == 0;
    let run = |args: &[&str]| -> bool {
        let mut cmd = if root { Command::new(args[0]) } else { Command::new("sudo") };
        if root {
            cmd.args(&args[1..]);
        } else {
            cmd.args(args);
        }
        cmd.status().map(|s| s.success()).unwrap_or(false)
    };
    eprintln!("w3bs: loading the vkms virtual display (a kernel module; sudo may ask for your password)");
    let loaded = run(&["modprobe", "vkms"]);
    let persisted = loaded && run(&["sh", "-c", "echo vkms > /etc/modules-load.d/w3bs-vkms.conf"]);
    if persisted {
        notes.push("loaded the vkms kernel module and kept it in /etc/modules-load.d/w3bs-vkms.conf".into());
    }
    for _ in 0..20 {
        if let Some(card) = find_vkms(sys) {
            return Ok(card);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(fail(
        "NO_VKMS",
        "the desktop needs the kernel's vkms virtual display; run: sudo modprobe vkms && echo vkms | sudo tee /etc/modules-load.d/w3bs-vkms.conf",
    ))
}

#[cfg(unix)]
fn gid_of(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.gid())
}
#[cfg(not(unix))]
fn gid_of(_: &Path) -> Option<u32> {
    None
}

fn build_image(tag: &str) -> Result<()> {
    let ctx = data_dir()?.join("build");
    std::fs::create_dir_all(&ctx).map_err(|e| fail("DESKTOP_BUILD", format!("{}: {e}", ctx.display())))?;
    std::fs::write(ctx.join("Dockerfile"), DOCKERFILE)
        .and_then(|_| std::fs::write(ctx.join("session.sh"), SESSION))
        .and_then(|_| std::fs::write(ctx.join("browser.sh"), BROWSER)).map_err(|e| fail("DESKTOP_BUILD", e.to_string()))?;
    eprintln!("w3bs: building the Kubuntu desktop image {tag} (first time only, a few minutes)");
    let ok = Command::new("docker")
        .args(["build", "--quiet", "-t", tag, "-t", &format!("{IMAGE}:latest")])
        .arg(&ctx)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(fail("DESKTOP_BUILD", format!("docker build failed; retry, or build by hand: docker build -t {tag} {}", ctx.display())));
    }
    Ok(())
}

/// Point this machine's fleet agent at the desktop (or away from it). Ok(None) when the
/// machine is not in a fleet.
fn share(screen: Option<Screen>) -> Result<Option<Value>> {
    let path = fleet::config_path(None)?;
    if fleet::load_config(&path)?.is_none() {
        return Ok(None);
    }
    fleet::screen_set(None, Some(screen)).map(Some)
}

pub fn start(control: bool, geometry: Option<&str>, rebuild: bool) -> Result<Value> {
    if let Some(g) = geometry {
        let ok = g.split_once('x').is_some_and(|(w, h)| w.parse::<u16>().is_ok_and(|w| w >= 640) && h.parse::<u16>().is_ok_and(|h| h >= 480));
        if !ok {
            return Err(Error::usage("BAD_GEOMETRY", format!("{g:?} is not WIDTHxHEIGHT, e.g. 1920x1080")));
        }
    }
    require_docker()?;
    let mut notes = Vec::new();
    let card = ensure_vkms(&mut notes)?;
    let tag = image_tag();
    if rebuild || docker(&["image", "inspect", &tag]).is_err() {
        build_image(&tag)?;
    }
    let dir = data_dir()?;
    for sub in ["home", "vnc"] {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(dir.join(sub)).map_err(|e| fail("DESKTOP_DIR", format!("{}: {e}", dir.join(sub).display())))?;
    }
    let sock = socket_path()?;
    let _ = std::fs::remove_file(&sock);
    let _ = docker(&["rm", "-f", CONTAINER]);
    let (uid, gid) = ids();
    let user = format!("{uid}:{gid}");
    let device = card.display().to_string();
    let card_gid = gid_of(&card).map(|g| g.to_string());
    let home = format!("{}:/home/desktop", dir.join("home").display());
    let vnc = format!("{}:/vnc", dir.join("vnc").display());
    let geometry_env = format!("W3BS_GEOMETRY={}", geometry.unwrap_or("1920x1080"));
    let mcp_env = format!("W3BS_MCP={MCP}");
    let mut args = vec!["run", "-d", "--name", CONTAINER, "--restart", "unless-stopped", "--network", "host", "--user", &user, "--shm-size", "512m", "--device", &device];
    if let Some(g) = card_gid.as_deref() {
        args.extend(["--group-add", g]);
    }
    args.extend(["-e", &geometry_env, "-e", &mcp_env, "-v", &home, "-v", &vnc, &tag]);
    docker(&args).map_err(|e| fail("DESKTOP_START", format!("docker run failed: {e}")))?;

    let started = Instant::now();
    while !sock.exists() {
        if started.elapsed() > START_TIMEOUT {
            return Err(fail("DESKTOP_START", format!("the desktop did not come up in {}s; see `w3bs fleet desktop logs`", START_TIMEOUT.as_secs())));
        }
        if docker(&["inspect", "-f", "{{.State.Running}}", CONTAINER]).as_deref() != Ok("true") && started.elapsed() > Duration::from_secs(5) {
            return Err(fail("DESKTOP_START", "the desktop container stopped; see `w3bs fleet desktop logs`"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let vnc_addr = format!("unix:{}", sock.display());
    let shared = share(Some(Screen { mode: if control { Mode::Control } else { Mode::View }, vnc: vnc_addr.clone() }))?;
    if shared.is_none() {
        notes.push(format!("this machine is not in a fleet, so nothing shares the screen yet; join it, then run: w3bs fleet screen on --vnc {vnc_addr}"));
    }
    Ok(json!({
        "container": CONTAINER, "image": tag, "display": device, "vnc": vnc_addr,
        "mcp": format!("http://{MCP}/mcp"), "home": dir.join("home"),
        "screen": shared.as_ref().map(|s| &s["screen"]), "url": shared.as_ref().map(|s| &s["url"]),
        "notes": notes,
    }))
}

pub fn stop() -> Result<Value> {
    require_docker()?;
    let removed = docker(&["rm", "-f", CONTAINER]).is_ok();
    // Stop sharing only if the agent was sharing this desktop.
    let path = fleet::config_path(None)?;
    let ours = format!("unix:{}", socket_path()?.display());
    let unshared = match fleet::load_config(&path)? {
        Some(c) if c.screen.as_ref().is_some_and(|s| s.vnc == ours) => share(None)?.is_some(),
        _ => false,
    };
    Ok(json!({ "container": CONTAINER, "removed": removed, "screen_off": unshared }))
}

pub fn status() -> Result<Value> {
    require_docker()?;
    let state = docker(&["inspect", "-f", "{{.State.Status}} {{.State.StartedAt}} {{.RestartCount}} {{.Config.Image}}", CONTAINER]).ok();
    let mut parts = state.as_deref().unwrap_or_default().split(' ');
    let sock = socket_path()?;
    let agent_screen = fleet::load_config(&fleet::config_path(None)?)?.and_then(|c| c.screen).map(|s| screen::info(&s));
    Ok(json!({
        "container": CONTAINER,
        "state": parts.next().filter(|s| !s.is_empty()).unwrap_or("absent"),
        "started_at": parts.next(), "restarts": parts.next(), "image": parts.next(),
        "current_image": image_tag(), "vnc": format!("unix:{}", sock.display()), "vnc_ready": sock.exists(),
        "mcp": format!("http://{MCP}/mcp"), "vkms": find_vkms(Path::new("/sys/class/drm")),
        "screen": agent_screen, "logs": data_dir()?.join("home/.local/state/w3bs-desktop/logs"),
    }))
}

pub fn logs() -> Result<Value> {
    require_docker()?;
    let out = Command::new("docker").args(["logs", "--tail", "40", CONTAINER]).output().map_err(|e| fail("DOCKER_UNAVAILABLE", e.to_string()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(json!({ "container": CONTAINER, "log": text, "session_logs": data_dir()?.join("home/.local/state/w3bs-desktop/logs") }))
}

/// Set (or with `off`, remove) the desktop's VNC password, read from stdin. The session gives it
/// to w0vncserver as a VncAuth password file in the desktop's home, so the w3bs.org page asks for
/// it; VNC uses only the first 8 characters. Restarts the desktop so it takes effect.
pub fn password(off: bool) -> Result<Value> {
    require_docker()?;
    let file = data_dir()?.join("home/.config/tigervnc/passwd");
    let tag = image_tag();
    let mut notes = Vec::new();
    if off {
        let _ = std::fs::remove_file(&file);
    } else {
        if docker(&["image", "inspect", &tag]).is_err() {
            return Err(fail("DESKTOP_IMAGE", "the desktop image is not built yet; run: w3bs fleet desktop start"));
        }
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| fail("PASSWORD_READ", e.to_string()))?;
        let pw = line.trim_end_matches(['\r', '\n']);
        if pw.is_empty() {
            return Err(Error::usage("EMPTY_PASSWORD", "give the password on stdin, e.g. read -rs p; printf '%s' \"$p\" | w3bs fleet desktop password"));
        }
        if pw.chars().count() > 8 {
            notes.push("VNC uses only the first 8 characters; typing the whole password still works".to_string());
        }
        let mut child = Command::new("docker")
            .args(["run", "--rm", "-i", "--network", "none", "--entrypoint", "vncpasswd", &tag, "-f"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| fail("DOCKER_UNAVAILABLE", e.to_string()))?;
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("piped stdin");
            stdin.write_all(pw.as_bytes()).map_err(|e| fail("DESKTOP_PASSWORD", e.to_string()))?;
        }
        let out = child.wait_with_output().map_err(|e| fail("DESKTOP_PASSWORD", e.to_string()))?;
        if !out.status.success() || out.stdout.is_empty() {
            return Err(fail("DESKTOP_PASSWORD", "vncpasswd failed in the desktop image; rebuild it: w3bs fleet desktop start --rebuild"));
        }
        let dir = file.parent().expect("passwd has a parent");
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(dir).map_err(|e| fail("DESKTOP_DIR", format!("{}: {e}", dir.display())))?;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        {
            use std::io::Write;
            o.open(&file).and_then(|mut f| f.write_all(&out.stdout)).map_err(|e| fail("DESKTOP_PASSWORD", format!("{}: {e}", file.display())))?;
        }
    }
    let running = docker(&["inspect", "-f", "{{.Config.Image}}", CONTAINER]).ok();
    let restarted = match running.as_deref() {
        Some(image) if image == tag => docker(&["restart", CONTAINER]).is_ok(),
        Some(_) => {
            notes.push("the desktop runs an older image that ignores the password; run: w3bs fleet desktop start".to_string());
            false
        }
        None => false,
    };
    Ok(json!({ "container": CONTAINER, "password": !off, "file": file, "restarted": restarted, "notes": notes }))
}

/// Register an agent on the desktop's layer and give the MCP line for it, with the token file
/// as seen from the host.
pub fn agent(name: &str, preset: &str) -> Result<Value> {
    require_docker()?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::usage("BAD_AGENT", "agent names are letters, digits, - and _"));
    }
    let (uid, _) = ids();
    let runtime = format!("XDG_RUNTIME_DIR=/tmp/runtime-{uid}");
    let out = docker(&["exec", "-e", &runtime, CONTAINER, "w3bs-desktop", "agents", "add", name, "--preset", preset, "--json"])
        .map_err(|e| fail("DESKTOP_AGENT", format!("could not register {name} (is the desktop running? `w3bs fleet desktop status`): {e}")))?;
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    let inner = v["token_file"].as_str().ok_or_else(|| fail("DESKTOP_AGENT", format!("unexpected answer from the desktop layer: {out}")))?;
    let host_token = data_dir()?.join("home").join(inner.trim_start_matches("/home/desktop/"));
    let claude = format!("claude mcp add --transport http w3bs-desktop http://{MCP}/mcp --header \"Authorization: Bearer $(cat '{}')\"", host_token.display());
    Ok(json!({ "agent": name, "preset": preset, "token_file": host_token, "mcp": format!("http://{MCP}/mcp"), "claude_code": claude }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_vkms_among_cards() {
        let dir = tempfile::tempdir().unwrap();
        let sys = dir.path().join("class/drm");
        let devs = dir.path().join("devices");
        for (card, dev) in [("card0", "pci0000:00/0000:00:02.0"), ("card1", "faux/vgem"), ("card2", "faux/vkms")] {
            std::fs::create_dir_all(devs.join(dev)).unwrap();
            std::fs::create_dir_all(sys.join(card)).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(devs.join(dev), sys.join(card).join("device")).unwrap();
        }
        std::fs::create_dir_all(sys.join("card2-Virtual-2")).unwrap();
        #[cfg(unix)]
        assert_eq!(find_vkms(&sys), Some(PathBuf::from("/dev/dri/card2")));
        assert_eq!(find_vkms(&dir.path().join("nothing")), None);
    }

    #[test]
    fn image_tag_follows_the_build_files() {
        let t = image_tag();
        assert!(t.starts_with("w3bs-kubuntu-desktop:"));
        assert_eq!(t.len(), "w3bs-kubuntu-desktop:".len() + 12);
        assert_eq!(t, image_tag());
        assert!(DOCKERFILE.contains("w0vncserver") && SESSION.contains("/vnc/vnc.sock"));
        // Screen capture needs KWin's screenshot check off and a Pictures folder; agents need a browser.
        assert!(SESSION.contains("KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1") && SESSION.contains("Pictures"));
        assert!(DOCKERFILE.contains("google-chrome-stable") && DOCKERFILE.contains("w3bs-browser") && BROWSER.contains("google-chrome"));
    }
}
