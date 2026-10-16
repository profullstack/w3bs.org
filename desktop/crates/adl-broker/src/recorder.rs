//! Desktop Training Mode: the recorder behind `w3bs desktop training` (W3BS Training PRD
//! sections 5 to 8).
//!
//! What it does, and what it deliberately does not:
//! - It starts Idle, after every daemon start, and returns to Idle whenever the layer is disabled
//!   (lock, suspend, logout, emergency stop, tray gone). Resuming always needs a fresh human
//!   action; nothing resumes on its own.
//! - Only the person starts or resumes it: the native control, the control app, or an interactive
//!   terminal outside any agent's process tree. Pausing, finishing, discarding and deleting are
//!   allowed from any control peer because they only reduce capture.
//! - It polls the focused window. A frame is kept only when that window belongs to an approved
//!   application and no unapproved window overlaps it; the frame is cropped to the window.
//!   Everything else becomes a recorded gap, never a guess.
//! - It records no input events, no typed text and no clipboard. On Wayland there is no passive
//!   input tap (InputCapture redirects input), so the bundle's profile is `visual-review` and
//!   actor origin is `unknown`.
//! - While it records, agents cannot take the input lease unless the person allowed it at start.
//! - Frames stay in memory. On finish the bundle is piped to `w3bs training import -`; if that
//!   fails it waits in memory for a retry and is lost if the daemon exits. It is never written
//!   to disk in plaintext.

use crate::broker::{Broker, Peer};
use crate::training_bundle::{self as tb, Display, FocusStep, Frame, Gap, RecordingData};
use crate::adapter::PixelRect;
use crate::util::{now_ms, random_id};
use adl_schema::*;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TrainingConfig {
    pub tick_ms: u64,
    /// Without a live stream: look at the same window this often (focus changes always look).
    pub keyframe_ms: u64,
    /// With a live stream: look at most this often, and only after the compositor sent new pixels.
    pub stream_eval_ms: u64,
    /// Keep a new frame of the same window only when at least this many pixels per thousand
    /// changed since the last kept frame: a blinking caret or a passing mouse pointer is not
    /// activity. Smaller changes add up against the last kept frame, so they are never lost.
    pub min_change_permille: u32,
    pub max_frames: usize,
    pub max_bytes: usize,
    pub max_pending: usize,
    /// Hand finished bundles to `w3bs training import`. Off in unit tests.
    pub handoff: bool,
    /// Where bundles waiting for W3BS Training are also kept so a restart of the desktop layer
    /// (an update) does not lose them. Must be memory-backed (Linux: the tmpfs runtime dir), so
    /// nothing reaches a disk unencrypted; None keeps them in this process only.
    pub spill_dir: Option<std::path::PathBuf>,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self { tick_ms: 500, keyframe_ms: 4_000, stream_eval_ms: 1_000, min_change_permille: 1, max_frames: 1_500, max_bytes: 256 * 1024 * 1024, max_pending: 4, handoff: true, spill_dir: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecState {
    Idle,
    Arming,
    Recording,
    Paused,
    Blocked,
    Finishing,
    Error,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct StartRequest {
    /// `observe` (my work) or `teach` (this task)
    pub mode: String,
    #[serde(default)]
    pub goal: Option<String>,
    /// Application ids (as `w3bs desktop windows list` shows them) whose windows may be captured.
    pub applications: Vec<String>,
    /// Let agents keep using the input lease while recording (steps become mixed-origin).
    #[serde(default)]
    pub allow_agents: bool,
}

struct Recording {
    data: RecordingData,
    t0: Instant,
    apps: Vec<String>,
    allow_agents: bool,
    last_focus: Option<(String, String)>,
    last_frame: Option<(u64, String)>,
    /// Pixels of the last kept frame, to tell real change from noise.
    last_px: Option<Pixels>,
    /// Reading displays from a live stream (portal ScreenCast) instead of screenshots.
    streaming: bool,
    /// (display id, stream sequence) last looked at: an unchanged sequence means no new pixels.
    last_seq: Option<(String, u64)>,
    last_eval: u64,
    open_gap: Option<Gap>,
    bytes: usize,
    errors: u32,
}

struct Pending {
    name: String,
    bundle: Vec<u8>,
    to_ms: u64,
    error: String,
}

pub(crate) struct RecInner {
    state: RecState,
    reason: Option<String>,
    rec: Option<Recording>,
    pending: Vec<Pending>,
    last: Option<Value>,
}

impl Default for RecInner {
    fn default() -> Self {
        Self { state: RecState::Idle, reason: None, rec: None, pending: vec![], last: None }
    }
}

fn in_scope(apps: &[String], w: &WindowInfo) -> bool {
    let id = w.app_id.to_lowercase();
    !id.is_empty() && apps.contains(&id)
}

fn open_gap(r: &mut Recording, kind: &'static str, ns: u64, detail: &str) {
    match &r.open_gap {
        Some(g) if g.kind == kind => {}
        _ => {
            close_gap(r, ns);
            r.open_gap = Some(Gap { kind, from_ns: ns, to_ns: None, detail: detail.into() });
        }
    }
}

fn close_gap(r: &mut Recording, ns: u64) {
    if let Some(mut g) = r.open_gap.take() {
        g.to_ns = Some(ns.max(g.from_ns));
        r.data.gaps.push(g);
    }
}

/// Raw pixels of a crop, in the colour type they will be encoded with.
#[derive(Clone)]
struct Pixels {
    width: u32,
    height: u32,
    color: png::ColorType,
    data: Vec<u8>,
}

impl Pixels {
    /// Pixels that differ from `other`, or None when the two cannot be compared.
    fn changed_from(&self, other: &Pixels) -> Option<usize> {
        if (self.width, self.height, self.color) != (other.width, other.height, other.color) {
            return None;
        }
        let n = self.color.samples();
        Some(self.data.chunks_exact(n).zip(other.data.chunks_exact(n)).filter(|(a, b)| a != b).count())
    }

    fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(self.color);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut wr = enc.write_header().map_err(|e| AdlError::internal(e.to_string()))?;
            wr.write_image_data(&self.data).map_err(|e| AdlError::internal(e.to_string()))?;
        }
        Ok(out)
    }
}

/// Crop a PNG to a pixel rectangle, keeping its colour type.
fn crop_png_pixels(png_bytes: &[u8], x: u32, y: u32, w: u32, h: u32) -> Result<Pixels> {
    let mut dec = png::Decoder::new(png_bytes);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| AdlError::internal(format!("decode capture: {e}")))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| AdlError::internal(format!("decode capture: {e}")))?;
    let channels = info.color_type.samples();
    let (fw, fh) = (info.width, info.height);
    let (x, y) = (x.min(fw.saturating_sub(1)), y.min(fh.saturating_sub(1)));
    let (w, h) = (w.min(fw - x).max(1), h.min(fh - y).max(1));
    let stride = info.line_size;
    let mut data = Vec::with_capacity((w * h) as usize * channels);
    for row in y..y + h {
        let start = row as usize * stride + x as usize * channels;
        data.extend_from_slice(&buf[start..start + w as usize * channels]);
    }
    Ok(Pixels { width: w, height: h, color: info.color_type, data })
}

#[cfg(test)]
fn crop_png(png_bytes: &[u8], x: u32, y: u32, w: u32, h: u32) -> Result<Vec<u8>> {
    crop_png_pixels(png_bytes, x, y, w, h)?.to_png()
}

/// Pipe a bundle to `w3bs training import -`. `W3BS_DESKTOP_TRAINING_HANDOFF` replaces the
/// command (run by `sh -c`, bundle on stdin, name in `W3BS_TRAINING_BUNDLE`) for rigs and for
/// people who keep W3BS Training somewhere unusual.
fn handoff(name: &str, bundle: &[u8]) -> std::result::Result<Value, String> {
    let mut cmd = if let Ok(sh) = std::env::var("W3BS_DESKTOP_TRAINING_HANDOFF") {
        let mut c = Command::new("sh");
        c.arg("-c").arg(sh);
        c
    } else if let Some(bin) = which("w3bs-training") {
        let mut c = Command::new(bin);
        c.args(["import", "-", "--filename", name]);
        c
    } else if let Some(bin) = which("w3bs") {
        let mut c = Command::new(bin);
        c.args(["training", "import", "-", "--filename", name]);
        c
    } else {
        return Err(if cfg!(windows) {
            "W3BS Training is not installed. Install it: irm https://w3bs.org/training/install.ps1 | iex".into()
        } else {
            "W3BS Training is not installed. Install it: curl -fsSL https://w3bs.org/training/install.sh | sh".into()
        });
    };
    cmd.env("W3BS_TRAINING_BUNDLE", name).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not start W3BS Training: {e}"))?;
    // A child that exits before reading (not installed, wrong program) closes the pipe; keep
    // going so its own words, not "Broken pipe", reach the person.
    let wrote = {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        stdin.write_all(bundle)
    };
    // Two minutes, plus a second for every 2 MB: a long recording takes a while to seal.
    let limit = 120 + (bundle.len() / (2 * 1024 * 1024)) as u64;
    let deadline = Instant::now() + Duration::from_secs(limit);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return Err(format!("W3BS Training did not finish the import within {limit} s"));
            }
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() && wrote.is_ok() {
        return Ok(serde_json::from_slice(&out.stdout).unwrap_or_else(|_| json!({"imported": true})));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.trim().is_empty() {
        let why = wrote.err().map(|e| e.to_string()).unwrap_or_else(|| format!("it exited with {}", out.status));
        return Err(format!("W3BS Training did not take the recording ({why}). {}", install_hint()));
    }
    Err(refusal(&err))
}

fn install_hint() -> &'static str {
    if cfg!(windows) {
        "Install it: irm https://w3bs.org/training/install.ps1 | iex"
    } else {
        "Install it: curl -fsSL https://w3bs.org/training/install.sh | sh"
    }
}

/// Turn W3BS Training's stderr into one sentence that says what to do next.
fn refusal(stderr: &str) -> String {
    // `w3bs training …` without w3bs-training installed: the w3bs CLI has no such command.
    if stderr.contains("unknown command") && stderr.contains("training") {
        return format!("W3BS Training is not installed. {}, then Try again.", install_hint());
    }
    let v = serde_json::from_str::<Value>(stderr.trim()).ok();
    let code = v.as_ref().and_then(|v| v["error"]["code"].as_str()).unwrap_or("");
    let msg = v.as_ref().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or_else(|| stderr.trim().chars().take(300).collect());
    if code == "KEY_UNAVAILABLE" {
        let fix = if cfg!(target_os = "linux") {
            "Install libsecret's secret-tool so it can use your desktop keyring (Debian/Ubuntu: sudo apt install libsecret-tools; Fedora: sudo dnf install libsecret; Arch: sudo pacman -S libsecret), then Try again."
        } else {
            "Unlock your keychain, then Try again."
        };
        return format!("W3BS Training has no key to encrypt the recording with. {fix}");
    }
    format!("W3BS Training refused the recording: {msg}")
}

/// Find a program on PATH or where the W3BS installers put it (a user service's PATH often lacks
/// ~/.local/bin; on Windows a PATH change reaches the broker only after it restarts).
pub fn which(bin: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(&h).join(".local/bin"));
        dirs.push(PathBuf::from(h).join(".w3bs/bin"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join("Programs").join(bin).join("bin"));
    }
    let name = if cfg!(windows) { format!("{bin}.exe") } else { bin.to_string() };
    dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

impl Broker {
    fn rec(&self) -> std::sync::MutexGuard<'_, RecInner> {
        self.rec.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn rec_event(&self) {
        let status = self.training_status();
        self.broadcast_event("training", status);
    }

    fn set_rec_state(&self, r: &mut RecInner, s: RecState, reason: Option<String>) {
        r.state = s;
        r.reason = reason;
        let blocks = matches!(s, RecState::Arming | RecState::Recording | RecState::Paused | RecState::Blocked) && r.rec.as_ref().is_some_and(|x| !x.allow_agents);
        self.rec_blocks_leases.store(blocks, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether a recording currently refuses agent input leases.
    pub fn recording_blocks_leases(&self) -> bool {
        self.rec_blocks_leases.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn training_status(&self) -> Value {
        let r = self.rec();
        let rec = r.rec.as_ref().map(|x| {
            json!({
                "mode": if x.data.teach { "teach" } else { "observe" },
                "goal": x.data.goal,
                "applications": x.apps,
                "allow_agents": x.allow_agents,
                "started_at": tb::iso(x.data.started_ms),
                "elapsed_ms": x.t0.elapsed().as_millis() as u64,
                "frames": x.data.frames.len(),
                "steps": x.data.steps.len(),
                "gaps": x.data.gaps.len() + usize::from(x.open_gap.is_some()),
                "excluded_now": x.open_gap.as_ref().map(|g| g.kind),
                "capture": if x.streaming { "screencast" } else { "screenshot" },
                "marks": x.data.marks.len(),
                "bytes": x.bytes,
            })
        });
        json!({
            "state": r.state,
            "reason": r.reason,
            "recording": rec,
            "pending_handoffs": r.pending.iter().map(|p| json!({"name": p.name, "bytes": p.bundle.len(), "error": p.error})).collect::<Vec<_>>(),
            "last_finished": r.last,
            "agent_leases": if self.recording_blocks_leases() { "refused while recording" } else { "unchanged" },
            "captures": "focused-window frames of approved applications only; no input events, typed text, clipboard or audio",
            "capture_profile": "visual-review",
        })
    }

    fn require_person(&self, peer: &Peer, op: &str) -> Result<()> {
        if peer.agent_descendant || !(peer.native_control || peer.interactive) {
            self.audit.log(op, None, None, "denied", "", json!({"pid": peer.pid}));
            return Err(AdlError::denied(format!("{op} needs the person: use the tray, the control app or your own terminal")));
        }
        Ok(())
    }

    /// Applications with open windows, for choosing what may be recorded (ids only, no titles).
    pub fn training_apps(&self) -> Result<Value> {
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AdlError::new(ErrorCode::Blocked, "turn the desktop layer on to list applications"));
        }
        let mut apps: std::collections::BTreeMap<String, (usize, bool)> = Default::default();
        for w in self.adapter.windows()? {
            let id = w.app_id.to_lowercase();
            if id.is_empty() {
                continue;
            }
            let e = apps.entry(id).or_default();
            e.0 += 1;
            e.1 |= w.focused;
        }
        Ok(Value::Array(apps.into_iter().map(|(id, (n, f))| json!({"app_id": id, "windows": n, "focused": f})).collect()))
    }

    pub fn training_tick_ms(&self) -> u64 {
        self.cfg.training.tick_ms
    }

    /// Observe my work / Teach this task.
    pub fn training_start(&self, peer: &Peer, req: StartRequest) -> Result<Value> {
        self.require_person(peer, "training.start")?;
        let teach = match req.mode.as_str() {
            "teach" => true,
            "observe" => false,
            other => return Err(AdlError::invalid(format!("mode must be observe or teach, not `{other}`"))),
        };
        let goal = req.goal.map(|g| g.trim().chars().take(500).collect::<String>()).filter(|g| !g.is_empty());
        if teach && goal.is_none() {
            return Err(AdlError::invalid("teach mode needs a goal: what are you about to do?"));
        }
        let apps: Vec<String> = req.applications.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
        if apps.is_empty() {
            return Err(AdlError::invalid("choose at least one application to record; whole-desktop recording is not offered"));
        }
        if apps.len() > 32 || apps.iter().any(|a| a.len() > 200) {
            return Err(AdlError::invalid("too many or too long application ids"));
        }
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AdlError::new(ErrorCode::Blocked, "turn the desktop layer on first: recording uses its screen access. Agents still cannot act while you record unless you allow them."));
        }
        {
            let mut r = self.rec();
            if !matches!(r.state, RecState::Idle | RecState::Error) || r.rec.is_some() {
                return Err(AdlError::new(ErrorCode::Busy, "a recording is already running; finish or discard it first"));
            }
            self.set_rec_state(&mut r, RecState::Arming, None);
        }
        let layout = self.adapter.layout();
        let displays = layout
            .as_ref()
            .map(|l| l.displays.iter().map(|d| Display { id: d.id.clone(), x: d.logical.x, y: d.logical.y, width: d.logical.width, height: d.logical.height, scale: d.scale }).collect())
            .unwrap_or_default();
        let session = random_id("ses", 8);
        let data = RecordingData {
            bundle_id: format!("bun_{}", &session[4..]),
            episode_ids: vec![],
            session_id: session,
            teach,
            goal,
            applications: apps.clone(),
            started_ms: now_ms(),
            ended_ns: 0,
            tick_ns: self.cfg.training.tick_ms * 1_000_000,
            frames: vec![],
            steps: vec![],
            gaps: vec![],
            marks: vec![],
            explanations: vec![],
            outcome: None,
            interrupted: false,
            displays,
            adapter: self.adapter.name(),
            producer_version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            revocation_generation: self.generation.load(std::sync::atomic::Ordering::SeqCst),
        };
        // Read displays from the live stream when the backend has one (no compositor round trip
        // per frame, and new pixels only when something changed); otherwise screenshots.
        let streaming = self.adapter.start_frame_stream();
        let rec = Recording { data, t0: Instant::now(), apps, allow_agents: req.allow_agents, last_focus: None, last_frame: None, last_px: None, streaming, last_seq: None, last_eval: 0, open_gap: None, bytes: 0, errors: 0 };
        {
            let mut r = self.rec();
            r.rec = Some(rec);
            self.set_rec_state(&mut r, RecState::Recording, None);
        }
        if !req.allow_agents {
            self.revoke_lease_for_recording();
        }
        self.audit.log("training.start", None, None, "allowed", if teach { "teach" } else { "observe" }, json!({"applications": req.applications.len(), "allow_agents": req.allow_agents}));
        self.rec_event();
        Ok(self.training_status())
    }

    pub fn training_pause(&self) -> Result<Value> {
        {
            let mut r = self.rec();
            if r.state != RecState::Recording {
                return Err(AdlError::invalid("not recording"));
            }
            if let Some(x) = r.rec.as_mut() {
                let ns = x.t0.elapsed().as_nanos() as u64;
                close_gap(x, ns);
                x.open_gap = Some(Gap { kind: "paused", from_ns: ns, to_ns: None, detail: "paused by the person".into() });
            }
            self.set_rec_state(&mut r, RecState::Paused, None);
        }
        self.audit.log("training.pause", None, None, "allowed", "", Value::Null);
        self.rec_event();
        Ok(self.training_status())
    }

    pub fn training_resume(&self, peer: &Peer) -> Result<Value> {
        self.require_person(peer, "training.resume")?;
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AdlError::new(ErrorCode::Blocked, "the desktop layer is off"));
        }
        {
            let mut r = self.rec();
            if !matches!(r.state, RecState::Paused | RecState::Blocked) {
                return Err(AdlError::invalid("nothing is paused"));
            }
            if r.state == RecState::Blocked && r.rec.as_ref().is_some_and(|x| x.data.frames.len() >= self.cfg.training.max_frames || x.bytes >= self.cfg.training.max_bytes) {
                return Err(AdlError::new(ErrorCode::Blocked, "this recording reached its size limit; finish it and start another"));
            }
            if let Some(x) = r.rec.as_mut() {
                close_gap(x, x.t0.elapsed().as_nanos() as u64);
            }
            self.set_rec_state(&mut r, RecState::Recording, None);
        }
        self.audit.log("training.resume", None, None, "allowed", "", Value::Null);
        self.rec_event();
        Ok(self.training_status())
    }

    /// Mark a task boundary (observe mode starts a new episode here).
    pub fn training_mark(&self) -> Result<Value> {
        let mut r = self.rec();
        let x = r.rec.as_mut().ok_or_else(|| AdlError::invalid("not recording"))?;
        let ns = x.t0.elapsed().as_nanos() as u64;
        x.data.marks.push(ns);
        drop(r);
        self.rec_event();
        Ok(self.training_status())
    }

    /// The person's own explanation, stored as a human-authored annotation.
    pub fn training_explain(&self, peer: &Peer, text: &str) -> Result<Value> {
        self.require_person(peer, "training.explain")?;
        let text: String = text.trim().chars().take(8_000).collect();
        if text.is_empty() {
            return Err(AdlError::invalid("the explanation is empty"));
        }
        let mut r = self.rec();
        let x = r.rec.as_mut().ok_or_else(|| AdlError::invalid("not recording"))?;
        if x.data.explanations.len() >= 32 {
            return Err(AdlError::invalid("too many explanations in one recording"));
        }
        x.data.explanations.push(text);
        drop(r);
        self.rec_event();
        Ok(self.training_status())
    }

    /// Finish and hand the recording to W3BS Training. `outcome` is the person's own verdict.
    pub fn training_finish(&self, outcome: Option<&str>) -> Result<Value> {
        if let Some(o) = outcome
            && !["success", "partial", "failure", "interrupted", "unknown"].contains(&o)
        {
            return Err(AdlError::invalid("outcome must be success, partial, failure, interrupted or unknown"));
        }
        self.finish_internal(outcome.map(String::from), false).ok_or_else(|| AdlError::invalid("not recording"))
    }

    /// Stop recording because the layer closed (disable, lock, suspend, emergency stop).
    pub(crate) fn training_abort(&self, why: &str) {
        if self.finish_internal(None, true).is_some() {
            self.audit.log("training.stop", None, None, "allowed", "interrupted", json!({"reason": why}));
        }
    }

    fn finish_internal(&self, outcome: Option<String>, interrupted: bool) -> Option<Value> {
        let mut rec = {
            let mut r = self.rec();
            let rec = r.rec.take()?;
            self.set_rec_state(&mut r, RecState::Finishing, None);
            rec
        };
        let ns = rec.t0.elapsed().as_nanos() as u64;
        // The end state counts: one last look keeps any change since the last kept frame, however
        // small. Not after an interruption: the layer is closed and must not capture.
        if !interrupted
            && self.open.load(std::sync::atomic::Ordering::SeqCst)
            && rec.open_gap.is_none()
            && let Ok(Some(w)) = self.adapter.focused_window()
            && in_scope(&rec.apps, &w)
        {
            let _ = self.capture_window(&mut rec, &w, ns, true, 0);
        }
        if rec.streaming {
            self.adapter.stop_frame_stream();
        }
        close_gap(&mut rec, ns);
        rec.data.ended_ns = ns.max(1);
        rec.data.outcome = outcome;
        rec.data.interrupted = interrupted;
        let n = tb::segments(&rec.data).len();
        rec.data.episode_ids = (0..n).map(|_| random_id("ep", 8)).collect();
        let bundle = tb::build(&rec.data);
        let name = format!("{}.w3bs-training.zip", rec.data.session_id);
        let summary = json!({
            "session_id": rec.data.session_id, "episodes": n, "frames": rec.data.frames.len(), "steps": rec.data.steps.len(),
            "gaps": rec.data.gaps.len(), "interrupted": interrupted, "finished_at": tb::iso(now_ms()),
        });
        drop(rec);
        let result = if self.cfg.training.handoff { handoff(&name, &bundle) } else { Err("handoff disabled".into()) };
        let mut r = self.rec();
        let last = match result {
            Ok(v) => json!({"summary": summary, "handoff": "imported", "import": v}),
            Err(e) => {
                eprintln!("w3bs-desktop: recording {name} was not handed to W3BS Training: {e}");
                if r.pending.len() >= self.cfg.training.max_pending {
                    let dropped = r.pending.remove(0);
                    self.unspill(&dropped.name);
                }
                let p = Pending { name, bundle, to_ms: now_ms(), error: e.clone() };
                let kept = self.spill(&p);
                r.pending.push(p);
                let note = if kept {
                    "Kept in memory (and the session's memory-backed runtime directory) until it is handed over; it survives a restart of the desktop layer, not a logout. Retry from the control app or `w3bs desktop training handoff`."
                } else {
                    "Held in memory only. Retry from the control app or `w3bs desktop training handoff`; it is lost if the daemon stops."
                };
                json!({"summary": summary, "handoff": "pending", "error": e, "note": note})
            }
        };
        r.last = Some(last.clone());
        self.set_rec_state(&mut r, RecState::Idle, if interrupted { Some("stopped because the desktop layer closed".into()) } else { None });
        drop(r);
        self.audit.log("training.finish", None, None, "allowed", last["handoff"].as_str().unwrap_or(""), json!({"frames": summary["frames"], "interrupted": interrupted}));
        self.rec_event();
        Some(last)
    }

    /// Keep a waiting bundle in the memory-backed spill directory. Returns whether it was kept.
    fn spill(&self, p: &Pending) -> bool {
        let Some(dir) = &self.cfg.training.spill_dir else { return false };
        let write = || -> std::io::Result<()> {
            #[cfg(unix)]
            {
                use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
                let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(dir.join(&p.name))?;
                std::io::Write::write_all(&mut f, &p.bundle)
            }
            #[cfg(not(unix))]
            {
                let _ = (dir, p);
                Err(std::io::Error::other("spilling is unix-only"))
            }
        };
        match write() {
            Ok(()) => true,
            Err(e) => {
                eprintln!("w3bs-desktop: could not keep {} across restarts: {e}", p.name);
                false
            }
        }
    }

    fn unspill(&self, name: &str) {
        if let Some(dir) = &self.cfg.training.spill_dir {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }

    /// Load bundles a previous run of the desktop layer left waiting. Called once at startup.
    pub fn training_restore_pending(&self) -> usize {
        let Some(dir) = &self.cfg.training.spill_dir else { return 0 };
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        let mut found: Vec<(u64, String, Vec<u8>)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                if !name.ends_with(".w3bs-training.zip") {
                    return None;
                }
                let ms = e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
                Some((ms, name, std::fs::read(e.path()).ok()?))
            })
            .collect();
        found.sort();
        let mut r = self.rec();
        let n = found.len();
        for (to_ms, name, bundle) in found {
            if r.pending.len() >= self.cfg.training.max_pending {
                let dropped = r.pending.remove(0);
                self.unspill(&dropped.name);
            }
            r.pending.push(Pending { name, bundle, to_ms, error: "kept across a restart of the desktop layer; not handed over yet".into() });
        }
        n
    }

    /// Throw the current recording away without handing anything over.
    pub fn training_discard(&self) -> Result<Value> {
        {
            let mut r = self.rec();
            if r.rec.take().is_none() {
                return Err(AdlError::invalid("not recording"));
            }
            self.set_rec_state(&mut r, RecState::Idle, None);
        }
        self.audit.log("training.discard", None, None, "allowed", "", Value::Null);
        self.rec_event();
        Ok(self.training_status())
    }

    /// Retry handing pending bundles to W3BS Training.
    pub fn training_handoff(&self) -> Result<Value> {
        let pending = std::mem::take(&mut self.rec().pending);
        let mut done = 0;
        let mut kept = vec![];
        for mut p in pending {
            match handoff(&p.name, &p.bundle) {
                Ok(_) => {
                    self.unspill(&p.name);
                    done += 1;
                }
                Err(e) => {
                    eprintln!("w3bs-desktop: recording {} was not handed to W3BS Training: {e}", p.name);
                    p.error = e;
                    kept.push(p);
                }
            }
        }
        let left = kept.len();
        self.rec().pending.splice(0..0, kept);
        self.rec_event();
        Ok(json!({"imported": done, "still_pending": left}))
    }

    /// Delete what the recorder still holds that was captured since `since_ms` (wall clock):
    /// frames and steps of the current recording, and pending bundles overlapping the window.
    /// Data already imported lives in W3BS Training: `w3bs training forget`.
    pub fn training_forget(&self, since_ms: u64) -> Result<Value> {
        let mut r = self.rec();
        let mut frames = 0;
        let mut steps = 0;
        if let Some(x) = r.rec.as_mut() {
            let cutoff = since_ms.saturating_sub(x.data.started_ms) * 1_000_000;
            let before = (x.data.frames.len(), x.data.steps.len());
            x.data.frames.retain(|f| f.ns < cutoff);
            x.data.steps.retain(|s| s.ns < cutoff);
            let kept: std::collections::HashSet<String> = x.data.frames.iter().map(|f| f.observation_id.clone()).collect();
            for s in &mut x.data.steps {
                s.before = s.before.take().filter(|o| kept.contains(o));
                s.after = s.after.take().filter(|o| kept.contains(o));
            }
            x.data.marks.retain(|m| *m < cutoff);
            x.bytes = x.data.frames.iter().map(|f| f.png.len()).sum();
            x.last_frame = x.data.frames.last().map(|f| (f.ns, f.observation_id.clone()));
            x.last_px = None;
            x.last_seq = None;
            frames = before.0 - x.data.frames.len();
            steps = before.1 - x.data.steps.len();
            let now = x.t0.elapsed().as_nanos() as u64;
            close_gap(x, now);
            if cutoff < now {
                x.data.gaps.push(Gap { kind: "other", from_ns: cutoff, to_ns: Some(now), detail: "recent activity deleted by the person".into() });
            }
        }
        let before = r.pending.len();
        let (keep, gone): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut r.pending).into_iter().partition(|p| p.to_ms < since_ms);
        for p in &gone {
            self.unspill(&p.name);
        }
        r.pending = keep;
        let dropped = before - r.pending.len();
        drop(r);
        self.audit.log("training.forget", None, None, "allowed", "", json!({"frames": frames, "steps": steps, "pending_bundles": dropped}));
        self.rec_event();
        Ok(json!({"frames_deleted": frames, "steps_deleted": steps, "pending_bundles_deleted": dropped, "next": "Recordings already handed over live in W3BS Training: run `w3bs training forget` for the same window."}))
    }

    /// One capture step. The daemon calls it every `tick_ms`; tests call it directly. The tray
    /// and the control app hear about it when capture stops or resumes (an unapproved window
    /// took focus or went away), not only on state changes.
    pub fn training_tick(&self) {
        let gap = || self.rec().rec.as_ref().and_then(|x| x.open_gap.as_ref().map(|g| g.kind));
        let before = gap();
        self.capture_tick();
        if gap() != before {
            self.rec_event();
        }
    }

    fn capture_tick(&self) {
        let mut r = self.rec();
        if r.state != RecState::Recording {
            return;
        }
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            drop(r);
            self.training_abort("desktop layer closed");
            return;
        }
        let lease_held = self.lease_held();
        let cfg = self.cfg.training.clone();
        let Some(x) = r.rec.as_mut() else { return };
        let ns = x.t0.elapsed().as_nanos() as u64;
        let w = match self.adapter.focused_window() {
            Ok(Some(w)) => w,
            Ok(None) => {
                open_gap(x, "other", ns, "no focused window");
                return;
            }
            Err(_) => {
                x.errors += 1;
                open_gap(x, "other", ns, "window state unavailable");
                if x.errors >= 20 {
                    self.set_rec_state(&mut r, RecState::Error, Some("the desktop stopped reporting windows".into()));
                    drop(r);
                    self.rec_event();
                }
                return;
            }
        };
        x.errors = 0;
        if !in_scope(&x.apps, &w) {
            // Which application it was is not recorded: it was not approved. Coming back to an
            // approved window afterwards counts as a focus change.
            x.last_focus = None;
            open_gap(x, "excluded_scope", ns, "a window outside the approved applications was focused");
            return;
        }
        let focus = (w.id.clone(), w.title.clone());
        let changed = x.last_focus.as_ref() != Some(&focus);
        // A live stream says when pixels changed, so look often but cheaply; screenshots cost a
        // compositor round trip each, so look only every keyframe interval.
        let every = if x.streaming { cfg.stream_eval_ms } else { cfg.keyframe_ms };
        let due = x.last_frame.is_none() || ns.saturating_sub(x.last_eval) >= every * 1_000_000;
        let before = x.last_frame.as_ref().map(|(_, id)| id.clone());
        let mut after = None;
        if changed || due {
            x.last_eval = ns;
            match self.capture_window(x, &w, ns, changed, cfg.min_change_permille) {
                Ok(Some(id)) => {
                    close_gap(x, ns);
                    after = Some(id);
                }
                Ok(None) => {}
                Err(_) => open_gap(x, "dropped_frames", ns, "the screen capture failed"),
            }
        }
        if changed {
            x.last_focus = Some(focus);
            let origin = if lease_held { "mixed" } else { "unknown" };
            x.data.steps.push(FocusStep { step_id: random_id("step", 8), ns, app: w.app_id.clone(), title: w.title.clone(), before, after, origin });
        }
        if x.data.frames.len() >= cfg.max_frames || x.bytes >= cfg.max_bytes {
            let ns = x.t0.elapsed().as_nanos() as u64;
            open_gap(x, "paused", ns, "size limit reached");
            self.set_rec_state(&mut r, RecState::Blocked, Some("this recording reached its size limit; finish it to keep what was captured".into()));
            drop(r);
            self.rec_event();
        }
    }

    /// Capture the focused window's display, refuse if an unapproved window overlaps it, crop to
    /// the window. Returns the frame that now shows the window: a new one when it changed enough
    /// (any change at all when `force`, e.g. a focus change), else the last kept one. None when
    /// nothing could be kept.
    fn capture_window(&self, x: &mut Recording, w: &WindowInfo, ns: u64, force: bool, min_permille: u32) -> Result<Option<String>> {
        let Some(b) = w.bounds else {
            open_gap(x, "other", ns, "the focused window reported no geometry");
            return Ok(None);
        };
        let layout = self.adapter.layout()?;
        let d = w
            .display
            .as_ref()
            .and_then(|id| layout.displays.iter().find(|d| &d.id == id))
            .or_else(|| layout.displays.iter().find(|d| d.logical.contains(b.x + b.width / 2.0, b.y + b.height / 2.0)))
            .ok_or_else(|| AdlError::not_found("no display for the window"))?
            .clone();
        let last_id = x.last_frame.as_ref().map(|(_, id)| id.clone());
        // With a stream, an unchanged sequence number means the compositor sent no new pixels:
        // nothing happened on this display, so there is nothing to look at.
        let probe = if x.streaming { self.adapter.stream_pixels(&d.id, None) } else { None };
        if let Some(p) = &probe
            && !force
            && x.open_gap.is_none()
            && x.last_seq.as_ref() == Some(&(d.id.clone(), p.seq))
        {
            return Ok(last_id);
        }
        // Strict scope: a 3x3 grid inside the window must show this window or another approved one.
        for i in 1..=3 {
            for j in 1..=3 {
                let (px, py) = (b.x + b.width * f64::from(i) / 4.0, b.y + b.height * f64::from(j) / 4.0);
                if let Ok(Some(top)) = self.adapter.window_at(px, py)
                    && top.id != w.id
                    && !in_scope(&x.apps, &top)
                {
                    open_gap(x, "excluded_scope", ns, "an unapproved window overlapped the approved one");
                    return Ok(None);
                }
            }
        }
        let gen0 = self.generation.load(std::sync::atomic::Ordering::SeqCst);
        // The window's rectangle in the display's pixels, for a frame of pw x ph pixels.
        let rect = |pw: u32, ph: u32| {
            let (sx, sy) = (f64::from(pw) / d.logical.width, f64::from(ph) / d.logical.height);
            let left = ((b.x.max(d.logical.x) - d.logical.x) * sx).floor().max(0.0) as u32;
            let top = ((b.y.max(d.logical.y) - d.logical.y) * sy).floor().max(0.0) as u32;
            let right = (((b.x + b.width).min(d.logical.x + d.logical.width) - d.logical.x) * sx).ceil().max(0.0) as u32;
            let bottom = (((b.y + b.height).min(d.logical.y + d.logical.height) - d.logical.y) * sy).ceil().max(0.0) as u32;
            (right > left && bottom > top).then_some(PixelRect { x: left, y: top, width: right - left, height: bottom - top })
        };
        let streamed = probe.and_then(|p| {
            let r = rect(p.width, p.height)?;
            let got = self.adapter.stream_pixels(&d.id, Some(r))?;
            let c = got.crop?;
            (got.rgba.len() == (c.width * c.height * 4) as usize).then_some((got.seq, Pixels { width: c.width, height: c.height, color: png::ColorType::Rgba, data: got.rgba }))
        });
        let px = match streamed {
            Some((seq, px)) => {
                x.last_seq = Some((d.id.clone(), seq));
                px
            }
            None => {
                let (png, pw, ph) = self.adapter.capture(&d.id)?;
                let Some(r) = rect(pw, ph) else {
                    open_gap(x, "other", ns, "the window is outside its display");
                    return Ok(None);
                };
                crop_png_pixels(&png, r.x, r.y, r.width, r.height)?
            }
        };
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) || self.generation.load(std::sync::atomic::Ordering::SeqCst) != gen0 {
            // The layer closed while capturing: discard the late frame.
            return Ok(None);
        }
        if let Some(last) = &x.last_px
            && let Some(changed) = px.changed_from(last)
        {
            let total = (px.width as usize * px.height as usize).max(1);
            if changed == 0 || (!force && changed * 1000 < total * min_permille as usize) {
                // Nothing (or only noise) changed since the last kept frame: it still shows the window.
                if let Some((t, _)) = x.last_frame.as_mut() {
                    *t = ns;
                }
                return Ok(last_id);
            }
        }
        let png = px.to_png()?;
        let id = random_id("obs", 8);
        x.bytes += png.len();
        x.data.frames.push(Frame { observation_id: id.clone(), artifact_id: random_id("art", 8), ns, png, width: px.width, height: px.height, app: w.app_id.clone(), title: w.title.clone() });
        x.last_frame = Some((ns, id.clone()));
        x.last_px = Some(px);
        Ok(Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::SessionEvent;
    use crate::audit::Audit;
    use crate::mock::MockDesktop;
    use crate::store::AgentStore;
    use crate::Config;
    use std::sync::Arc;

    #[cfg(unix)]
    #[test]
    fn handoff_to_a_child_that_exits_early_says_why() {
        // The only test that runs handoff(); the others disable it, so the env var is safe.
        unsafe { std::env::set_var("W3BS_DESKTOP_TRAINING_HANDOFF", "echo 'w3bs: unknown command `training` (no `w3bs-training` on PATH)' >&2; exit 2") };
        let big = vec![0u8; 4 * 1024 * 1024];
        let e = handoff("x.w3bs-training.zip", &big).unwrap_err();
        assert!(e.starts_with("W3BS Training is not installed."), "{e}");
        assert!(!e.contains("Broken pipe"), "{e}");
        unsafe { std::env::set_var("W3BS_DESKTOP_TRAINING_HANDOFF", "exec 0<&-; exit 3") };
        let e = handoff("x.w3bs-training.zip", &big).unwrap_err();
        assert!(e.contains("did not take the recording") && e.contains("install.sh"), "{e}");
        unsafe { std::env::remove_var("W3BS_DESKTOP_TRAINING_HANDOFF") };
    }

    #[test]
    fn refusals_name_the_fix() {
        let key = r#"{"error":{"code":"KEY_UNAVAILABLE","message":"No protected key store is available."}}"#;
        assert!(refusal(key).starts_with("W3BS Training has no key"));
        assert_eq!(refusal(key).contains("libsecret-tools"), cfg!(target_os = "linux"));
        assert_eq!(refusal(r#"{"error":{"code":"BAD","message":"nope"}}"#), "W3BS Training refused the recording: nope");
    }

    fn broker() -> (Arc<Broker>, Arc<MockDesktop>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockDesktop::new());
        let cfg = Config {
            pace_ms: 0,
            require_native_control: false,
            training: TrainingConfig { keyframe_ms: 0, stream_eval_ms: 0, handoff: false, ..TrainingConfig::default() },
            ..Config::default()
        };
        let b = Arc::new(Broker::new(mock.clone(), AgentStore::open(dir.path().join("agents.json")).unwrap(), Audit::open(dir.path().join("audit"), 7), cfg));
        (b, mock, dir)
    }
    fn person() -> Peer {
        Peer { pid: Some(1), agent_descendant: false, interactive: true, native_control: false }
    }
    fn enable(b: &Broker) {
        let peer = Peer { native_control: true, ..person() };
        let ch = b.enable_request(&peer).unwrap();
        b.enable_confirm(&peer, &ch.code.unwrap()).unwrap();
    }
    fn teach(b: &Broker, apps: &[&str]) -> Result<Value> {
        b.training_start(&person(), StartRequest { mode: "teach".into(), goal: Some("Write the release notes".into()), applications: apps.iter().map(|s| s.to_string()).collect(), allow_agents: false })
    }
    fn state(b: &Broker) -> String {
        b.training_status()["state"].as_str().unwrap().to_string()
    }

    #[test]
    fn starts_idle_and_only_a_person_can_start() {
        let (b, _m, _d) = broker();
        assert_eq!(state(&b), "idle");
        assert_eq!(teach(&b, &["org.kde.kate"]).unwrap_err().code, ErrorCode::Blocked, "layer off");
        enable(&b);
        let agent = Peer { agent_descendant: true, ..person() };
        let req = || StartRequest { mode: "observe".into(), goal: None, applications: vec!["org.kde.kate".into()], allow_agents: false };
        assert_eq!(b.training_start(&agent, req()).unwrap_err().code, ErrorCode::Denied);
        let pipe = Peer { interactive: false, ..person() };
        assert_eq!(b.training_start(&pipe, req()).unwrap_err().code, ErrorCode::Denied);
        assert_eq!(b.training_start(&person(), StartRequest { applications: vec![], ..req() }).unwrap_err().code, ErrorCode::Invalid);
        assert!(b.training_start(&person(), StartRequest { mode: "teach".into(), ..req() }).is_err(), "teach needs a goal");
        b.training_start(&person(), req()).unwrap();
        assert_eq!(state(&b), "recording");
        assert_eq!(b.training_start(&person(), req()).unwrap_err().code, ErrorCode::Busy);
    }

    #[test]
    fn captures_only_approved_unoccluded_windows_cropped() {
        let (b, m, _d) = broker();
        enable(&b);
        teach(&b, &["org.kde.kate"]).unwrap();
        // Kate is focused and topmost.
        b.training_tick();
        m.focus("w-browser"); // unapproved app takes focus
        b.training_tick();
        m.focus("w-editor"); // back, but Firefox no longer overlaps (Kate is topmost again)
        b.training_tick();
        let s = b.training_status();
        assert_eq!(s["recording"]["frames"], 1, "the same unchanged window is deduplicated");
        assert_eq!(s["recording"]["steps"], 2);
        let last = b.training_finish(Some("success")).unwrap();
        assert_eq!(last["handoff"], "pending");
        assert_eq!(state(&b), "idle");
        let st = b.training_status();
        assert_eq!(st["pending_handoffs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_live_stream_keeps_frames_only_when_the_window_really_changed() {
        let (b, m, _d) = broker();
        m.offer_frame_stream(true);
        enable(&b);
        teach(&b, &["org.kde.kate"]).unwrap();
        assert_eq!(m.frame_stream_state(), (true, 1));
        let frames = |b: &Broker| b.training_status()["recording"]["frames"].as_u64().unwrap();
        // Kate is 800x600 at (100,100) on DP-1 (scale 1): 480,000 pixels, so 1/1000 is 480.
        let px = |x, y, w, h| PixelRect { x, y, width: w, height: h };
        b.training_tick();
        assert_eq!(frames(&b), 1, "the first look keeps a frame");
        for _ in 0..3 {
            b.training_tick();
        }
        assert_eq!(frames(&b), 1, "no new pixels from the compositor: nothing to keep");
        m.repaint();
        b.training_tick();
        assert_eq!(frames(&b), 1, "a new frame with identical pixels is not activity");
        m.paint("DP-1", px(1000, 800, 200, 200), 0);
        b.training_tick();
        assert_eq!(frames(&b), 1, "a change outside the approved window is not kept or looked at");
        m.paint("DP-1", px(200, 200, 10, 10), 0);
        b.training_tick();
        assert_eq!(frames(&b), 1, "100 changed pixels (a caret, a pointer) is noise");
        m.paint("DP-1", px(300, 300, 30, 30), 0);
        b.training_tick();
        assert_eq!(frames(&b), 2, "changes add up against the last kept frame and are kept");
        m.paint("DP-1", px(400, 400, 5, 5), 0);
        b.training_tick();
        assert_eq!(frames(&b), 2);
        let last = b.training_finish(Some("success")).unwrap();
        assert_eq!(last["summary"]["frames"], 3, "the end state is kept however small the change");
        assert_eq!(m.frame_stream_state(), (false, 1), "the stream stops with the recording");
    }

    #[cfg(unix)]
    #[test]
    fn waiting_recordings_survive_a_restart_through_the_spill_dir() {
        let spill = tempfile::tempdir().unwrap();
        let make = |dir: &std::path::Path| {
            let mock = Arc::new(MockDesktop::new());
            let cfg = Config {
                pace_ms: 0,
                require_native_control: false,
                training: TrainingConfig { keyframe_ms: 0, handoff: false, spill_dir: Some(spill.path().join("pending")), ..TrainingConfig::default() },
                ..Config::default()
            };
            Arc::new(Broker::new(mock, AgentStore::open(dir.join("agents.json")).unwrap(), Audit::open(dir.join("audit"), 7), cfg))
        };
        let d1 = tempfile::tempdir().unwrap();
        let b = make(d1.path());
        enable(&b);
        teach(&b, &["org.kde.kate"]).unwrap();
        b.training_tick();
        let last = b.training_finish(Some("success")).unwrap();
        assert_eq!(last["handoff"], "pending");
        assert!(last["note"].as_str().unwrap().contains("survives a restart"), "{last}");
        let files: Vec<_> = std::fs::read_dir(spill.path().join("pending")).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(files[0].metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(b);
        // A new broker (the updated desktop layer) finds it waiting.
        let d2 = tempfile::tempdir().unwrap();
        let b2 = make(d2.path());
        assert_eq!(b2.training_restore_pending(), 1);
        let st = b2.training_status();
        assert_eq!(st["pending_handoffs"].as_array().unwrap().len(), 1);
        assert!(st["pending_handoffs"][0]["error"].as_str().unwrap().contains("restart"));
        // Deleting recent activity deletes the kept copy too.
        b2.training_forget(0).unwrap();
        assert_eq!(std::fs::read_dir(spill.path().join("pending")).unwrap().count(), 0);
    }

    #[test]
    fn without_a_stream_small_changes_are_noise_too() {
        let (b, m, _d) = broker();
        enable(&b);
        teach(&b, &["org.kde.kate"]).unwrap();
        assert_eq!(m.frame_stream_state(), (false, 0));
        for _ in 0..3 {
            b.training_tick();
        }
        assert_eq!(b.training_status()["recording"]["frames"], 1, "screenshots of an unchanged window keep one frame");
    }

    #[test]
    fn overlap_by_an_unapproved_window_is_a_gap_not_a_frame() {
        let (b, m, _d) = broker();
        enable(&b);
        // Approve Firefox only; Kate (unapproved) sits on top of part of Firefox.
        m.focus("w-editor");
        let r = b.training_start(&person(), StartRequest { mode: "observe".into(), goal: None, applications: vec!["org.mozilla.firefox".into()], allow_agents: false });
        r.unwrap();
        // Focus Firefox without raising it above Kate: simulate by focusing then re-raising Kate.
        m.focus("w-browser");
        m.focus("w-editor");
        m.set_focused_only("w-browser");
        b.training_tick();
        let s = b.training_status();
        assert_eq!(s["recording"]["frames"], 0);
        assert_eq!(s["recording"]["excluded_now"], "excluded_scope");
    }

    #[test]
    fn agents_cannot_take_the_lease_while_teaching() {
        let (b, _m, _d) = broker();
        enable(&b);
        teach(&b, &["org.kde.kate"]).unwrap();
        assert!(b.recording_blocks_leases());
        b.training_pause().unwrap();
        assert!(b.recording_blocks_leases(), "paused still refuses agents");
        b.training_discard().unwrap();
        assert!(!b.recording_blocks_leases());
    }

    #[test]
    fn disable_lock_and_emergency_stop_end_the_recording() {
        for event in [SessionEvent::Locked, SessionEvent::Suspending, SessionEvent::EmergencyStop] {
            let (b, _m, _d) = broker();
            enable(&b);
            teach(&b, &["org.kde.kate"]).unwrap();
            b.training_tick();
            b.handle_session_event(event.clone());
            let s = b.training_status();
            assert_eq!(s["state"], "idle", "{event:?}");
            assert_eq!(s["last_finished"]["summary"]["interrupted"], true);
            // Nothing resumes on its own: a tick does nothing, and the layer is off.
            b.training_tick();
            assert_eq!(state(&b), "idle");
            assert!(b.training_resume(&person()).is_err());
        }
    }

    #[test]
    fn pause_resume_and_forget() {
        let (b, m, _d) = broker();
        enable(&b);
        teach(&b, &["org.kde.kate", "org.mozilla.firefox"]).unwrap();
        b.training_tick();
        b.training_pause().unwrap();
        b.training_tick();
        assert_eq!(b.training_status()["recording"]["frames"], 1, "nothing captured while paused");
        assert!(b.training_resume(&Peer { agent_descendant: true, ..person() }).is_err());
        b.training_resume(&person()).unwrap();
        m.focus("w-browser");
        b.training_tick();
        assert_eq!(b.training_status()["recording"]["frames"], 2);
        let r = b.training_forget(0).unwrap();
        assert_eq!(r["frames_deleted"], 2);
        assert_eq!(b.training_status()["recording"]["frames"], 0);
    }

    #[test]
    fn a_finished_recording_is_a_valid_bundle_shape() {
        let (b, m, _d) = broker();
        enable(&b);
        teach(&b, &["org.kde.kate", "org.mozilla.firefox"]).unwrap();
        b.training_tick();
        m.focus("w-browser");
        b.training_tick();
        b.training_explain(&person(), "Copy the version line first.").unwrap();
        b.training_finish(Some("success")).unwrap();
        let bundle = b.rec().pending[0].bundle.clone();
        // Central directory lists every file; the PNG frames are cropped to the windows.
        let text = String::from_utf8_lossy(&bundle);
        for name in ["manifest.json", "sessions.jsonl", "episodes.jsonl", "observations.jsonl", "actions.jsonl", "artifacts.jsonl", "rights.json", "checksums.json"] {
            assert!(text.contains(name), "{name}");
        }
        assert!(text.contains("\"capture_profile\":\"visual-review\""));
        assert!(text.contains("Copy the version line first."));
        assert!(!text.contains("Console"), "never records the unapproved terminal");
    }

    #[test]
    fn crop_keeps_the_requested_rectangle() {
        let png = crate::training_bundle::tests::png(10, 8, 7);
        let out = crop_png(&png, 2, 1, 5, 4).unwrap();
        let dec = png::Decoder::new(out.as_slice()).read_info().unwrap();
        assert_eq!((dec.info().width, dec.info().height), (5, 4));
    }
}
