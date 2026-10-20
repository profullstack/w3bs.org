//! The authorization and task broker (PRD sections 4, 7, 9, 11 and 12).
//!
//! Safety properties this module is responsible for:
//! - It starts Disabled, and only a human control peer can enable it.
//! - The gate is an atomic flag plus a revocation generation, so `disable` rejects new work
//!   without waiting on any lock held by in-flight input.
//! - Authorization is re-checked before *every* primitive input event, not when work is queued.
//! - Only one principal holds the desktop input lease; dispatch is serialized.
//! - Synthetic held buttons and keys are tracked and released on cancel, failure and stop.
//! - Observation results computed across a disable are discarded instead of returned.

use crate::adapter::{Adapter, InputEvent, SessionEvent};
use crate::audit::Audit;
use crate::store::AgentStore;
use crate::util::{now_ms, random_id, sha256_hex};
use adl_schema::coords::CaptureMeta;
use adl_schema::limits::*;
use adl_schema::*;
use base64::Engine as _;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    /// Pause between interpolated drag/scroll/click events. 0 in tests.
    pub pace_ms: u64,
    /// Refuse to enable, and disable, when no native control (tray/panel/TUI) is attached.
    pub require_native_control: bool,
    /// Non-desktop instances (containers, VMs, CI): no human sits at this screen, so agent access
    /// starts Enabled and `enable` needs no confirmation code. Implies no native control.
    pub enabled_by_default: bool,
    pub approval_ttl_ms: u64,
    pub capture_ttl_ms: u64,
    pub enable_challenge_ttl_ms: u64,
    pub default_actions_per_minute: u32,
    pub max_tasks_per_agent: usize,
    /// How long `stop` waits for in-flight dispatch to notice cancellation.
    pub stop_wait_ms: u64,
    /// Desktop Training Mode recorder settings.
    pub training: crate::recorder::TrainingConfig,
    /// Ask the person before an agent takes the input lease ("drive prompt"). Off on
    /// non-desktop instances (`enabled_by_default`), where nobody is there to answer.
    pub drive_prompt: bool,
    /// How long a drive prompt waits for an answer before it lapses (the agent then asks again).
    pub drive_request_ttl_ms: u64,
    /// Default "Postpone", and how long "Deny" holds before the agent may ask again.
    pub drive_postpone_ms: u64,
    /// "Allow" lasts while the agent keeps driving; this long without a lease and it asks again.
    pub drive_idle_ms: u64,
    /// How long a step handed to the person (CAPTCHA, 2FA, sign-in) waits before it lapses.
    pub human_step_ttl_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pace_ms: 8,
            require_native_control: true,
            enabled_by_default: false,
            approval_ttl_ms: 5 * 60_000,
            capture_ttl_ms: 60_000,
            enable_challenge_ttl_ms: 120_000,
            default_actions_per_minute: 600,
            max_tasks_per_agent: 8,
            stop_wait_ms: 1_000,
            training: crate::recorder::TrainingConfig::default(),
            drive_prompt: true,
            drive_request_ttl_ms: 10 * 60_000,
            drive_postpone_ms: 10 * 60_000,
            drive_idle_ms: 10 * 60_000,
            human_step_ttl_ms: 15 * 60_000,
        }
    }
}

/// An authenticated agent connection.
#[derive(Clone, Debug)]
pub struct Principal {
    pub agent: String,
    pub session: String,
}

/// What the daemon knows about a control-socket peer.
#[derive(Clone, Debug, Default)]
pub struct Peer {
    pub pid: Option<u32>,
    /// The peer's process tree includes an agent client (e.g. the app that launched our MCP server).
    pub agent_descendant: bool,
    /// The peer's stdin is an interactive terminal.
    pub interactive: bool,
    /// The peer identified itself as a native control and holds a subscription.
    pub native_control: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct EnableChallenge {
    pub expires_at_ms: u64,
    /// Present only when the requester is the native control itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub delivered_to: Vec<String>,
}

struct Lease {
    info: LeaseInfo,
}

struct Task {
    info: TaskInfo,
    /// Connection that began it; its tasks end when that connection closes.
    session: String,
}

#[derive(Clone)]
enum Held {
    Button(Button),
    Key(Key),
}

impl Held {
    fn name(&self) -> String {
        match self {
            Held::Button(b) => format!("button:{}", format!("{b:?}").to_lowercase()),
            Held::Key(k) => format!("key:{k}"),
        }
    }
    fn release(&self) -> InputEvent {
        match self {
            Held::Button(b) => InputEvent::Button { button: *b, pressed: false },
            Held::Key(k) => InputEvent::Key { key: k.clone(), pressed: false },
        }
    }
}

struct Inner {
    state: GlobalState,
    blocked_reason: Option<String>,
    lease: Option<Lease>,
    tasks: HashMap<String, Task>,
    approvals: HashMap<String, ApprovalInfo>,
    drive: HashMap<String, DriveRequest>,
    human_steps: HashMap<String, HumanStep>,
    captures: HashMap<String, CaptureMeta>,
    held: Vec<(Held, u64)>,
    challenge: Option<(String, u64)>,
    rate: HashMap<String, VecDeque<u64>>,
    native_controls: usize,
    subscribers: Vec<Sender<String>>,
    agent_roots: HashSet<u32>,
}

pub struct Broker {
    pub(crate) adapter: Arc<dyn Adapter>,
    pub store: AgentStore,
    pub audit: Audit,
    pub(crate) cfg: Config,
    pub(crate) open: AtomicBool,
    pub(crate) generation: AtomicU64,
    inner: Mutex<Inner>,
    dispatch: Mutex<()>,
    pub(crate) rec: Mutex<crate::recorder::RecInner>,
    pub(crate) rec_blocks_leases: AtomicBool,
}

/// Outcome of executing one action, for result accounting.
enum Fail {
    /// Nothing was emitted for this action (validation or authorization).
    Before(AdlError),
    /// Some events may have been delivered.
    During(AdlError),
}

impl Broker {
    pub fn new(adapter: Arc<dyn Adapter>, store: AgentStore, audit: Audit, cfg: Config) -> Self {
        Self {
            adapter,
            store,
            audit,
            cfg,
            open: AtomicBool::new(false),
            generation: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                state: GlobalState::Disabled,
                blocked_reason: None,
                lease: None,
                tasks: HashMap::new(),
                approvals: HashMap::new(),
                drive: HashMap::new(),
                human_steps: HashMap::new(),
                captures: HashMap::new(),
                held: vec![],
                challenge: None,
                rate: HashMap::new(),
                native_controls: 0,
                subscribers: vec![],
                agent_roots: HashSet::new(),
            }),
            dispatch: Mutex::new(()),
            rec: Mutex::new(crate::recorder::RecInner::default()),
            rec_blocks_leases: AtomicBool::new(false),
        }
    }

    /// Push an event to every control subscriber (tray, panel, control app).
    pub(crate) fn broadcast_event(&self, event: &str, data: serde_json::Value) {
        let mut g = self.lock();
        Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": event, "data": data}));
    }

    pub(crate) fn lease_held(&self) -> bool {
        self.lock().lease.as_ref().is_some_and(|l| l.info.expires_at_ms > now_ms())
    }

    /// A deliberate demonstration starts: whoever holds the input lease loses it.
    pub(crate) fn revoke_lease_for_recording(&self) {
        let mut g = self.lock();
        if let Some(l) = g.lease.take() {
            Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "lease_revoked", "data": {"agent": l.info.agent, "reason": "a person started teaching a task"}}));
            drop(g);
            self.audit.log("lease.revoke", Some(&l.info.agent), Some(&l.info.task), "allowed", "", json!({"reason": "training"}));
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn adapter_name(&self) -> String {
        self.adapter.name()
    }

    pub fn state(&self) -> GlobalState {
        self.lock().state
    }

    fn set_state(&self, g: &mut Inner, s: GlobalState, reason: Option<String>) {
        g.state = s;
        g.blocked_reason = if s == GlobalState::Blocked { reason.clone() } else { None };
        let ev = json!({"v": SCHEMA_VERSION, "event": "state", "data": {"state": s, "reason": reason}});
        Self::broadcast(g, &ev);
    }

    fn broadcast(g: &mut Inner, ev: &serde_json::Value) {
        let line = ev.to_string();
        g.subscribers.retain(|tx| tx.send(line.clone()).is_ok());
    }

    // ---------------------------------------------------------------- human control surface

    /// Subscribe to state/approval/enable events. Native controls also count toward the
    /// safety-control requirement until `detach_native_control`.
    pub fn subscribe(&self, tx: Sender<String>, native_control: bool) {
        let mut g = self.lock();
        let snapshot = json!({"v": SCHEMA_VERSION, "event": "state", "data": {"state": g.state, "reason": g.blocked_reason}});
        let _ = tx.send(snapshot.to_string());
        g.subscribers.push(tx);
        if native_control {
            g.native_controls += 1;
        }
    }

    /// How many native controls (tray, panel extension, terminal control) are attached.
    pub fn native_controls(&self) -> usize {
        self.lock().native_controls
    }

    /// Whether enabling needs a native control attached (false for `--enabled-by-default` and
    /// headless test brokers).
    pub fn requires_native_control(&self) -> bool {
        self.cfg.require_native_control && !self.cfg.enabled_by_default
    }

    pub fn detach_native_control(&self) {
        let lost = {
            let mut g = self.lock();
            g.native_controls = g.native_controls.saturating_sub(1);
            g.native_controls == 0 && self.cfg.require_native_control && !self.cfg.enabled_by_default && matches!(g.state, GlobalState::Enabled | GlobalState::Enabling)
        };
        if lost {
            self.disable("native control lost");
        }
    }

    /// Record the process that launched an agent connection, so its descendants cannot use
    /// human-only control operations.
    pub fn register_agent_root(&self, pid: u32) {
        self.lock().agent_roots.insert(pid);
    }

    pub fn is_agent_process(&self, ancestors: &[u32]) -> bool {
        let g = self.lock();
        ancestors.iter().any(|p| g.agent_roots.contains(p))
    }

    fn require_human(&self, peer: &Peer, op: &str) -> Result<()> {
        if peer.agent_descendant {
            self.audit.log(op, None, None, "denied", "", json!({"why": "agent process tree", "pid": peer.pid}));
            return Err(AdlError::denied(format!("{op} must come from the human, not from an agent's process tree")));
        }
        Ok(())
    }

    /// Enable without a confirmation code. Only for brokers started with `enabled_by_default`.
    pub fn enable_unattended(&self) -> Result<Status> {
        if !self.cfg.enabled_by_default {
            return Err(AdlError::denied("this broker needs a human to enable it (start it with --enabled-by-default for non-desktop instances)"));
        }
        {
            let mut g = self.lock();
            match g.state {
                GlobalState::Enabled => return Ok({ drop(g); self.status() }),
                GlobalState::Stopping => return Err(AdlError::new(ErrorCode::Busy, "still stopping")),
                _ => {}
            }
            self.set_state(&mut g, GlobalState::Enabling, None);
        }
        if let Err(e) = self.adapter.begin() {
            let mut g = self.lock();
            self.set_state(&mut g, GlobalState::Blocked, Some(e.message.clone()));
            self.audit.log("enable", None, None, "blocked", "", json!({"reason": e.message, "unattended": true}));
            return Err(e);
        }
        {
            let mut g = self.lock();
            g.challenge = None;
            self.open.store(true, Ordering::SeqCst);
            self.set_state(&mut g, GlobalState::Enabled, None);
        }
        self.audit.log("enable", None, None, "allowed", "enabled", json!({"unattended": true}));
        Ok(self.status())
    }

    pub fn enabled_by_default(&self) -> bool {
        self.cfg.enabled_by_default
    }

    /// Step 1 of enabling: check readiness and issue a confirmation code to the human.
    pub fn enable_request(&self, peer: &Peer) -> Result<EnableChallenge> {
        self.require_human(peer, "enable.request")?;
        let mut g = self.lock();
        match g.state {
            GlobalState::Enabled => return Err(AdlError::invalid("already enabled")),
            GlobalState::Stopping => return Err(AdlError::new(ErrorCode::Busy, "still stopping")),
            _ => {}
        }
        if self.cfg.require_native_control && g.native_controls == 0 {
            let why = "no native control is running (tray, panel extension or `w3bs desktop control`)".to_string();
            self.set_state(&mut g, GlobalState::Blocked, Some(why.clone()));
            return Err(AdlError::new(ErrorCode::Blocked, why));
        }
        self.set_state(&mut g, GlobalState::Enabling, None);
        drop(g);
        if let Err(e) = self.adapter.begin() {
            let mut g = self.lock();
            self.set_state(&mut g, GlobalState::Blocked, Some(e.message.clone()));
            self.audit.log("enable", None, None, "blocked", "", json!({"reason": e.message}));
            return Err(e);
        }
        let mut code_bytes = [0u8; 4];
        getrandom::fill(&mut code_bytes).expect("OS randomness unavailable");
        let code = format!("{:06}", u32::from_le_bytes(code_bytes) % 1_000_000);
        let expires = now_ms() + self.cfg.enable_challenge_ttl_ms;
        let mut g = self.lock();
        g.challenge = Some((code.clone(), expires));
        let mut delivered = vec![];
        if g.native_controls > 0 {
            Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "enable_requested", "data": {"code": code, "expires_at_ms": expires}}));
            delivered.push("native_control".into());
        }
        // The code goes only to the native control. A CLI confirmation has to type the code the
        // human sees there, which shows the person in front of the screen approved it.
        let give = peer.native_control;
        if give {
            delivered.push("requester".into());
        }
        Ok(EnableChallenge { expires_at_ms: expires, code: give.then_some(code), delivered_to: delivered })
    }

    /// Step 2: the human confirms with the code.
    pub fn enable_confirm(&self, peer: &Peer, code: &str) -> Result<Status> {
        self.require_human(peer, "enable.confirm")?;
        if !peer.native_control && !peer.interactive {
            return Err(AdlError::denied("confirm from the native control or an interactive terminal"));
        }
        {
            let mut g = self.lock();
            let ok = matches!(&g.challenge, Some((c, exp)) if crate::util::ct_eq(c.as_bytes(), code.trim().as_bytes()) && *exp > now_ms());
            if g.state != GlobalState::Enabling || !ok {
                return Err(AdlError::denied("no matching enable request; run enable again"));
            }
            g.challenge = None;
            self.open.store(true, Ordering::SeqCst);
            self.set_state(&mut g, GlobalState::Enabled, None);
        }
        self.audit.log("enable", None, None, "allowed", "enabled", serde_json::Value::Null);
        Ok(self.status())
    }

    /// Stop all agent activity but stay Enabled; agents must acquire a new lease explicitly.
    pub fn stop_all(&self, reason: &str) -> StopReport {
        self.stop_internal(reason, GlobalState::Enabled)
    }

    /// Close the whole layer. Always allowed, from anyone, in any state.
    pub fn disable(&self, reason: &str) -> StopReport {
        self.stop_internal(reason, GlobalState::Disabled)
    }

    /// The human took over: disable, and never resume without reauthorization.
    pub fn takeover(&self) -> StopReport {
        self.disable("human takeover")
    }

    fn stop_internal(&self, reason: &str, end_state: GlobalState) -> StopReport {
        let t0 = Instant::now();
        let was_open = self.open.swap(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let gate_closed_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let mut report = StopReport { reason: reason.into(), gate_closed_ms, ..Default::default() };
        {
            let mut g = self.lock();
            g.challenge = None;
            self.set_state(&mut g, GlobalState::Stopping, None);
            report.leases_revoked = g.lease.take().map_or(0, |_| 1);
            let now = now_ms();
            for t in g.tasks.values_mut().filter(|t| t.info.state == TaskState::Running) {
                t.info.state = TaskState::Cancelled;
                t.info.ended_at_ms = Some(now);
                report.tasks_cancelled += 1;
            }
            for a in g.approvals.values_mut().filter(|a| a.status == ApprovalStatus::Pending || a.status == ApprovalStatus::Approved) {
                a.status = ApprovalStatus::Expired;
                report.approvals_withdrawn += 1;
            }
            // Stop cancels every task, so every drive answer is moot; a new task asks again.
            let withdrawn: Vec<String> = g.drive.values().filter(|d| d.status == DriveStatus::Pending).map(|d| d.id.clone()).collect();
            report.approvals_withdrawn += withdrawn.len();
            g.drive.clear();
            for id in withdrawn {
                Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "drive_decided", "data": {"id": id, "status": DriveStatus::Expired}}));
            }
            // The tasks that asked for a human step are gone too.
            let steps: Vec<String> = g.human_steps.values().filter(|h| h.status == HumanStepStatus::Pending).map(|h| h.id.clone()).collect();
            for id in steps {
                let h = g.human_steps.get_mut(&id).unwrap();
                h.status = HumanStepStatus::Expired;
                h.resolved_at_ms = Some(now);
                Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "human_step_resolved", "data": {"id": id, "status": HumanStepStatus::Expired}}));
            }
            g.captures.clear();
        }
        report.streams_closed = self.adapter.close_streams();

        // Wait (bounded) for in-flight dispatch to observe the new generation and exit.
        let deadline = Instant::now() + Duration::from_millis(self.cfg.stop_wait_ms);
        let mut guard = None;
        while Instant::now() < deadline {
            match self.dispatch.try_lock() {
                Ok(gd) => {
                    guard = Some(gd);
                    break;
                }
                Err(std::sync::TryLockError::Poisoned(p)) => {
                    guard = Some(p.into_inner());
                    break;
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    report.batches_interrupted = 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        if guard.is_none() {
            report.cleanup_failures.push(format!("input dispatch did not stop within {} ms; the adapter may be hung", self.cfg.stop_wait_ms));
        }
        report.synthetic_inputs_released = self.release_held(&mut report.cleanup_failures);
        drop(guard);

        if end_state == GlobalState::Disabled {
            self.adapter.end();
        }
        {
            let mut g = self.lock();
            if end_state == GlobalState::Enabled && was_open {
                self.open.store(true, Ordering::SeqCst);
                self.set_state(&mut g, GlobalState::Enabled, None);
            } else {
                self.set_state(&mut g, GlobalState::Disabled, None);
            }
        }
        // A recording ends with the layer: it never outlives Disable, lock or suspend.
        if end_state == GlobalState::Disabled {
            self.training_abort(reason);
        }
        report.cleanup_ms = t0.elapsed().as_secs_f64() * 1000.0;
        report.note = "Disabling stops new work. It cannot retract input already delivered, unsend messages, or erase data an agent already received.".into();
        self.audit.log(
            if end_state == GlobalState::Disabled { "disable" } else { "stop" },
            None,
            None,
            "allowed",
            if report.cleanup_failures.is_empty() { "completed" } else { "cleanup_failed" },
            json!({"reason": reason, "leases": report.leases_revoked, "tasks": report.tasks_cancelled, "released": report.synthetic_inputs_released.len()}),
        );
        report
    }

    /// Release synthetic buttons/keys this layer is holding. Never touches physical state.
    fn release_held(&self, failures: &mut Vec<String>) -> Vec<String> {
        let held: Vec<(Held, u64)> = std::mem::take(&mut self.lock().held);
        let mut released = vec![];
        for (h, _) in held.into_iter().rev() {
            match self.adapter.emit(&h.release()) {
                Ok(()) => released.push(h.name()),
                Err(e) => failures.push(format!("release {}: {e}", h.name())),
            }
        }
        released
    }

    /// React to OS session events (lock, suspend, takeover, permission loss).
    pub fn pump_session_events(&self) {
        for e in self.adapter.poll_events() {
            self.handle_session_event(e);
        }
    }

    pub fn handle_session_event(&self, e: SessionEvent) {
        let active = matches!(self.state(), GlobalState::Enabled | GlobalState::Enabling);
        match e {
            SessionEvent::Locked if active => drop(self.disable("session locked")),
            SessionEvent::Suspending if active => drop(self.disable("system suspending")),
            SessionEvent::LoggingOut if active => drop(self.disable("logging out")),
            SessionEvent::EmergencyStop => drop(self.disable("emergency stop")),
            SessionEvent::PermissionLost(p) if active => drop(self.disable(&format!("permission lost: {p}"))),
            SessionEvent::AdapterFailed(why) if active => drop(self.disable(&format!("adapter failed: {why}"))),
            SessionEvent::PhysicalInput => {
                // Only an active agent session counts as a takeover; ordinary use while idle is fine,
                // and so is the person doing a step an agent handed them (CAPTCHA, 2FA, sign-in).
                let busy = self.lock().lease.is_some() || self.dispatch.try_lock().is_err();
                if active && busy && !self.human_step_open() {
                    drop(self.takeover());
                }
            }
            SessionEvent::LayoutChanged => {
                let mut g = self.lock();
                g.captures.clear();
                Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "layout_changed", "data": {}}));
            }
            _ => {}
        }
    }

    pub fn status(&self) -> Status {
        let g = self.lock();
        Status {
            schema: SCHEMA_VERSION,
            state: g.state,
            blocked_reason: g.blocked_reason.clone(),
            adapter: self.adapter.name(),
            lease: g.lease.as_ref().filter(|l| l.info.expires_at_ms > now_ms()).map(|l| l.info.clone()),
            running_tasks: g.tasks.values().filter(|t| t.info.state == TaskState::Running).map(|t| t.info.clone()).collect(),
            pending_approvals: g.approvals.values().filter(|a| a.status == ApprovalStatus::Pending).count(),
            pending_drive_requests: g.drive.values().filter(|d| d.status == DriveStatus::Pending && d.expires_at_ms > now_ms()).count(),
            drive_always_allow: self.store.drive_always_allow(),
            pending_human_steps: g.human_steps.values().filter(|h| h.status == HumanStepStatus::Pending && h.expires_at_ms > now_ms()).count(),
            agents: self.store.list().iter().filter(|a| !a.revoked).count(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    pub fn capabilities(&self) -> Vec<CapabilityReport> {
        self.adapter.capabilities()
    }

    pub fn tasks(&self) -> Vec<TaskInfo> {
        let mut v: Vec<_> = self.lock().tasks.values().map(|t| t.info.clone()).collect();
        v.sort_by_key(|t| t.created_at_ms);
        v
    }

    pub fn task(&self, id: &str) -> Result<TaskInfo> {
        self.lock().tasks.get(id).map(|t| t.info.clone()).ok_or_else(|| AdlError::not_found(format!("no task {id}")))
    }

    /// Cancel a task (human or owning agent). Its lease, if any, is revoked.
    pub fn cancel_task(&self, id: &str, by: Option<&Principal>) -> Result<TaskInfo> {
        let mut g = self.lock();
        let t = g.tasks.get_mut(id).ok_or_else(|| AdlError::not_found(format!("no task {id}")))?;
        if let Some(p) = by
            && t.info.agent != p.agent {
                return Err(AdlError::denied("task belongs to another agent"));
            }
        if t.info.state == TaskState::Running {
            t.info.state = TaskState::Cancelled;
            t.info.ended_at_ms = Some(now_ms());
        }
        let info = t.info.clone();
        if g.lease.as_ref().is_some_and(|l| l.info.task == id) {
            g.lease = None;
        }
        drop(g);
        self.audit.log("task.cancel", Some(&info.agent), Some(id), "allowed", "cancelled", serde_json::Value::Null);
        Ok(info)
    }

    pub fn approvals(&self) -> Vec<ApprovalInfo> {
        let now = now_ms();
        let mut g = self.lock();
        for a in g.approvals.values_mut() {
            if a.status == ApprovalStatus::Pending && a.expires_at_ms <= now {
                a.status = ApprovalStatus::Expired;
            }
        }
        let mut v: Vec<_> = g.approvals.values().cloned().collect();
        v.sort_by_key(|a| a.created_at_ms);
        v
    }

    pub fn decide_approval(&self, peer: &Peer, id: &str, approve: bool) -> Result<ApprovalInfo> {
        self.require_human(peer, "approvals.decide")?;
        let mut g = self.lock();
        let a = g.approvals.get_mut(id).ok_or_else(|| AdlError::not_found(format!("no approval {id}")))?;
        if a.status != ApprovalStatus::Pending || a.expires_at_ms <= now_ms() {
            return Err(AdlError::invalid(format!("approval is {:?}", a.status).to_lowercase()));
        }
        a.status = if approve { ApprovalStatus::Approved } else { ApprovalStatus::Denied };
        let info = a.clone();
        Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "approval_decided", "data": {"id": id, "status": info.status}}));
        drop(g);
        self.audit.log("approvals.decide", Some(&info.agent), Some(&info.task), if approve { "approved" } else { "denied" }, "", json!({"operation": info.operation}));
        Ok(info)
    }

    pub fn add_agent(&self, peer: &Peer, name: &str, policy: Policy) -> Result<(AgentInfo, String)> {
        self.require_human(peer, "agents.add")?;
        let r = self.store.add(name, policy)?;
        self.audit.log("agents.add", Some(&r.0.id), None, "allowed", "", json!({"name": name, "mode": r.0.policy.mode, "capabilities": r.0.policy.capabilities}));
        Ok(r)
    }

    pub fn update_agent(&self, peer: &Peer, id: &str, policy: Policy) -> Result<AgentInfo> {
        self.require_human(peer, "agents.update")?;
        let a = self.store.update_policy(id, policy)?;
        self.revoke_agent_activity(id);
        self.audit.log("agents.update", Some(id), None, "allowed", "", json!({"mode": a.policy.mode, "capabilities": a.policy.capabilities}));
        Ok(a)
    }

    /// Revoking only reduces access, so any peer may do it.
    pub fn revoke_agent(&self, id: &str) -> Result<()> {
        self.store.revoke(id)?;
        self.revoke_agent_activity(id);
        self.audit.log("agents.revoke", Some(id), None, "allowed", "", serde_json::Value::Null);
        Ok(())
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        self.revoke_agent(id).ok();
        self.store.remove(id)
    }

    fn revoke_agent_activity(&self, agent: &str) {
        let mut g = self.lock();
        if g.lease.as_ref().is_some_and(|l| l.info.agent == agent) {
            g.lease = None;
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        let now = now_ms();
        for t in g.tasks.values_mut().filter(|t| t.info.agent == agent && t.info.state == TaskState::Running) {
            t.info.state = TaskState::Cancelled;
            t.info.ended_at_ms = Some(now);
        }
    }

    // ---------------------------------------------------------------- agent surface

    pub fn authenticate(&self, token: &str, client_root_pid: Option<u32>) -> Result<Principal> {
        let a = self.store.authenticate(token)?;
        if let Some(pid) = client_root_pid {
            self.register_agent_root(pid);
        }
        self.audit.log("hello", Some(&a.id), None, "allowed", "", serde_json::Value::Null);
        Ok(Principal { agent: a.id, session: random_id("sess", 8) })
    }

    fn policy(&self, p: &Principal) -> Result<Policy> {
        let a = self.store.get(&p.agent).ok_or_else(|| AdlError::new(ErrorCode::Unauthenticated, "agent no longer exists"))?;
        if a.revoked {
            return Err(AdlError::new(ErrorCode::Unauthenticated, "agent has been revoked"));
        }
        if a.policy.expires_at_ms.is_some_and(|e| e <= now_ms()) {
            return Err(AdlError::denied("grant has expired"));
        }
        Ok(a.policy)
    }

    fn require_enabled(&self) -> Result<u64> {
        let g = self.generation.load(Ordering::SeqCst);
        if !self.open.load(Ordering::SeqCst) {
            return Err(AdlError::disabled());
        }
        Ok(g)
    }

    fn require_cap(&self, p: &Principal, cap: Capability) -> Result<Policy> {
        let pol = self.policy(p)?;
        if !pol.capabilities.contains(&cap) {
            let e = AdlError::denied(format!("agent is not granted {}", serde_json::to_value(cap).unwrap().as_str().unwrap()));
            self.audit.log("authorize", Some(&p.agent), None, "denied", "", json!({"capability": cap}));
            return Err(e);
        }
        Ok(pol)
    }

    /// Discard results computed across a disable/stop (no late sensitive results).
    fn still_valid(&self, gen0: u64) -> Result<()> {
        if !self.open.load(Ordering::SeqCst) || self.generation.load(Ordering::SeqCst) != gen0 {
            return Err(AdlError::disabled());
        }
        Ok(())
    }

    fn observe<T>(&self, data: T, generation: u64) -> Observation<T> {
        let now = now_ms();
        Observation { observed_at_ms: now, expires_at_ms: now + self.cfg.capture_ttl_ms, layout_generation: generation, backend: self.adapter.name(), data }
    }

    pub fn layout(&self, p: &Principal) -> Result<Observation<Layout>> {
        let gen0 = self.require_enabled()?;
        let pol = self.policy(p)?;
        if ![Capability::Pointer, Capability::ScreenCapture, Capability::Windows].iter().any(|c| pol.capabilities.contains(c)) {
            return Err(AdlError::denied("display metadata needs pointer, screen_capture or windows"));
        }
        let l = self.adapter.layout()?;
        self.still_valid(gen0)?;
        let g = l.generation;
        Ok(self.observe(l, g))
    }

    pub fn windows(&self, p: &Principal) -> Result<Observation<Vec<WindowInfo>>> {
        let gen0 = self.require_enabled()?;
        let pol = self.require_cap(p, Capability::Windows)?;
        let ws: Vec<_> = self.adapter.windows()?.into_iter().filter(|w| pol.allows_app(&w.app_id)).collect();
        let lg = self.adapter.layout()?.generation;
        self.still_valid(gen0)?;
        self.audit.log("windows", Some(&p.agent), None, "allowed", "completed", json!({"count": ws.len()}));
        Ok(self.observe(ws, lg))
    }

    pub fn inspect(&self, p: &Principal, window: &str) -> Result<Observation<AccessNode>> {
        let gen0 = self.require_enabled()?;
        let pol = self.require_cap(p, Capability::Accessibility)?;
        let w = self.adapter.windows()?.into_iter().find(|w| w.id == window).ok_or_else(|| AdlError::not_found(format!("no window {window}")))?;
        if !pol.allows_app(&w.app_id) {
            return Err(AdlError::denied(format!("{} is not in this agent's application grant", w.app_id)));
        }
        let node = self.adapter.inspect(window)?;
        let lg = self.adapter.layout()?.generation;
        self.still_valid(gen0)?;
        self.audit.log("inspect", Some(&p.agent), None, "allowed", "completed", json!({"app": w.app_id}));
        Ok(self.observe(node, lg))
    }

    pub fn screenshot(&self, p: &Principal, display: &str) -> Result<Screenshot> {
        let gen0 = self.require_enabled()?;
        let pol = self.require_cap(p, Capability::ScreenCapture)?;
        if pol.mode != ControlMode::BroadDesktop {
            return Err(AdlError::denied("display capture shows every application; it needs broad_desktop control"));
        }
        let layout = self.adapter.layout()?;
        let d = layout.displays.iter().find(|d| d.id == display).ok_or_else(|| AdlError::not_found(format!("no display {display}")))?.clone();
        let (png, w, h) = self.adapter.capture(display)?;
        self.still_valid(gen0)?;
        let now = now_ms();
        let meta = CaptureMeta {
            capture: random_id("cap", 8), display: d.id.clone(), pixel_width: w, pixel_height: h, logical: d.logical,
            layout_generation: layout.generation, taken_at_ms: now, expires_at_ms: now + self.cfg.capture_ttl_ms,
        };
        {
            let mut g = self.lock();
            g.captures.retain(|_, c| c.expires_at_ms > now);
            g.captures.insert(meta.capture.clone(), meta.clone());
        }
        self.audit.log("screenshot", Some(&p.agent), None, "allowed", "completed", json!({"display": display}));
        Ok(Screenshot { meta, png_base64: base64::engine::general_purpose::STANDARD.encode(png) })
    }

    pub fn pointer_position(&self, p: &Principal) -> Result<Observation<Option<(f64, f64)>>> {
        let gen0 = self.require_enabled()?;
        let pol = self.require_cap(p, Capability::Pointer)?;
        if pol.mode != ControlMode::BroadDesktop {
            return Err(AdlError::denied("pointer position needs broad_desktop control"));
        }
        let pos = self.adapter.pointer_position()?;
        if pos.is_none() {
            return Err(AdlError::unsupported("this backend cannot report the pointer position"));
        }
        let lg = self.adapter.layout()?.generation;
        self.still_valid(gen0)?;
        Ok(self.observe(pos, lg))
    }

    pub fn task_begin(&self, p: &Principal, description: &str) -> Result<TaskInfo> {
        self.require_enabled()?;
        self.policy(p)?;
        let mut g = self.lock();
        let running = g.tasks.values().filter(|t| t.info.agent == p.agent && t.info.state == TaskState::Running).count();
        if running >= self.cfg.max_tasks_per_agent {
            return Err(AdlError::new(ErrorCode::Busy, format!("agent already has {running} running tasks")));
        }
        let info = TaskInfo {
            id: random_id("task", 8), agent: p.agent.clone(), description: description.chars().take(200).collect(), state: TaskState::Running,
            created_at_ms: now_ms(), ended_at_ms: None, batches_completed: 0, batches_cancelled: 0, batches_unknown: 0,
        };
        g.tasks.insert(info.id.clone(), Task { info: info.clone(), session: p.session.clone() });
        drop(g);
        self.audit.log("task.begin", Some(&p.agent), Some(&info.id), "allowed", "", serde_json::Value::Null);
        Ok(info)
    }

    pub fn task_end(&self, p: &Principal, id: &str, outcome: TaskState) -> Result<TaskInfo> {
        let mut g = self.lock();
        let t = g.tasks.get_mut(id).ok_or_else(|| AdlError::not_found(format!("no task {id}")))?;
        if t.info.agent != p.agent {
            return Err(AdlError::denied("task belongs to another agent"));
        }
        if t.info.state == TaskState::Running {
            t.info.state = if outcome == TaskState::Running { TaskState::Completed } else { outcome };
            t.info.ended_at_ms = Some(now_ms());
        }
        let info = t.info.clone();
        if g.lease.as_ref().is_some_and(|l| l.info.task == id) {
            g.lease = None;
        }
        Ok(info)
    }

    /// An agent connection closed (client exited, timed out or crashed): cancel the tasks it
    /// began and free the input lease if one of them held it, so nothing keeps running unowned.
    pub fn end_session(&self, p: &Principal) -> usize {
        let mut g = self.lock();
        let now = now_ms();
        let mut ended = vec![];
        for t in g.tasks.values_mut().filter(|t| t.session == p.session && t.info.state == TaskState::Running) {
            t.info.state = TaskState::Cancelled;
            t.info.ended_at_ms = Some(now);
            ended.push(t.info.id.clone());
        }
        if g.lease.as_ref().is_some_and(|l| ended.contains(&l.info.task)) {
            g.lease = None;
            Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "lease", "data": null}));
        }
        drop(g);
        if !ended.is_empty() {
            self.audit.log("session.end", Some(&p.agent), None, "allowed", "cancelled", json!({"tasks": ended.len()}));
        }
        ended.len()
    }

    pub fn tasks_for(&self, p: &Principal) -> Vec<TaskInfo> {
        self.tasks().into_iter().filter(|t| t.agent == p.agent).collect()
    }

    pub fn lease_acquire(&self, p: &Principal, task: &str, ttl_ms: Option<u64>) -> Result<LeaseInfo> {
        let lease_gen = self.require_enabled()?;
        let pol = self.policy(p)?;
        if !pol.capabilities.contains(&Capability::Pointer) && !pol.capabilities.contains(&Capability::Keyboard) {
            return Err(AdlError::denied("the input lease needs pointer or keyboard access"));
        }
        if self.recording_blocks_leases() {
            return Err(AdlError::new(ErrorCode::Busy, "a person is recording a demonstration; agent input is paused until they finish"));
        }
        if let Some(h) = self.open_human_step() {
            return Err(AdlError::new(ErrorCode::Busy, "a person is doing a step by hand; agent input is paused until they mark it done")
                .with_details(json!({"human_step_id": h.id, "kind": h.kind})));
        }
        let ttl = ttl_ms.unwrap_or(DEFAULT_LEASE_TTL_MS).clamp(1_000, MAX_LEASE_TTL_MS);
        let now = now_ms();
        let mut g = self.lock();
        match g.tasks.get(task) {
            Some(t) if t.info.agent == p.agent && t.info.state == TaskState::Running => {}
            Some(t) if t.info.agent != p.agent => return Err(AdlError::denied("task belongs to another agent")),
            Some(_) => return Err(AdlError::new(ErrorCode::Cancelled, "task is not running")),
            None => return Err(AdlError::not_found(format!("no task {task}"))),
        }
        if let Some(l) = &g.lease
            && l.info.expires_at_ms > now && l.info.generation == lease_gen && (l.info.agent != p.agent || l.info.task != task) {
                return Err(AdlError::new(ErrorCode::Busy, "another task holds the desktop input lease").with_details(json!({"holder": l.info.agent, "expires_at_ms": l.info.expires_at_ms})));
            }
        let auto = self.drive_gate(&mut g, p, task, now)?;
        let info = LeaseInfo { id: random_id("lease", 8), agent: p.agent.clone(), task: task.into(), expires_at_ms: now + ttl, generation: lease_gen };
        g.lease = Some(Lease { info: info.clone() });
        Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "lease", "data": info}));
        drop(g);
        self.audit.log("lease.acquire", Some(&p.agent), Some(task), "allowed", "", json!({"ttl_ms": ttl, "drive": if auto { "always_allow" } else { "allowed" }}));
        Ok(info)
    }

    // ---------------------------------------------------------------- human steps

    /// The pending step an agent handed the person, if any (expired ones lapse here).
    fn open_human_step(&self) -> Option<HumanStep> {
        let now = now_ms();
        let mut g = self.lock();
        Self::expire_human_steps(&mut g, now);
        g.human_steps.values().find(|h| h.status == HumanStepStatus::Pending).cloned()
    }

    fn human_step_open(&self) -> bool {
        self.open_human_step().is_some()
    }

    fn expire_human_steps(g: &mut MutexGuard<'_, Inner>, now: u64) {
        let lapsed: Vec<String> = g.human_steps.values().filter(|h| h.status == HumanStepStatus::Pending && h.expires_at_ms <= now).map(|h| h.id.clone()).collect();
        for id in lapsed {
            let h = g.human_steps.get_mut(&id).unwrap();
            h.status = HumanStepStatus::Expired;
            h.resolved_at_ms = Some(now);
            Self::broadcast(g, &json!({"v": SCHEMA_VERSION, "event": "human_step_resolved", "data": {"id": id, "status": HumanStepStatus::Expired}}));
        }
    }

    /// An agent hands one step to the person: a CAPTCHA, a 2FA code, a sign-in, a payment. Agents
    /// do not solve "are you human" checks; they ask. Asking revokes the input lease, no
    /// agent may take it until the step is resolved, and the person's own input meanwhile is not a
    /// takeover. One open step per agent: asking again updates it.
    pub fn request_human_step(&self, p: &Principal, task: Option<&str>, kind: HumanStepKind, message: &str) -> Result<HumanStep> {
        self.require_enabled()?;
        let message: String = message.trim().chars().take(500).collect();
        if message.is_empty() {
            return Err(AdlError::invalid("say what the person should do (message)"));
        }
        let now = now_ms();
        let agent_name = self.store.get(&p.agent).map(|a| a.name).unwrap_or_else(|| p.agent.clone());
        let mut g = self.lock();
        Self::expire_human_steps(&mut g, now);
        if let Some(t) = task {
            match g.tasks.get(t) {
                Some(ti) if ti.info.agent == p.agent => {}
                _ => return Err(AdlError::not_found(format!("no task {t}"))),
            }
        }
        // The person needs the whole desktop, so whoever holds the lease gives it up.
        if g.lease.take().is_some() {
            Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "lease", "data": null}));
        }
        let existing = g.human_steps.values().find(|h| h.agent == p.agent && h.status == HumanStepStatus::Pending).map(|h| h.id.clone());
        let h = HumanStep {
            id: existing.unwrap_or_else(|| random_id("human", 8)),
            agent: p.agent.clone(),
            agent_name,
            task: task.map(Into::into),
            kind,
            message,
            status: HumanStepStatus::Pending,
            created_at_ms: now,
            expires_at_ms: now + self.cfg.human_step_ttl_ms,
            resolved_at_ms: None,
        };
        g.human_steps.insert(h.id.clone(), h.clone());
        Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "human_step_requested", "data": h}));
        drop(g);
        // The message is agent text about the user's accounts; the audit keeps only the kind.
        self.audit.log("human_step.request", Some(&p.agent), task, "approval_required", "", json!({"human_step": h.id, "kind": kind}));
        Ok(h)
    }

    /// An agent checks on a step it handed the person.
    pub fn human_step_status(&self, p: &Principal, id: &str) -> Result<HumanStep> {
        let mut g = self.lock();
        Self::expire_human_steps(&mut g, now_ms());
        match g.human_steps.get(id) {
            Some(h) if h.agent == p.agent => Ok(h.clone()),
            _ => Err(AdlError::not_found(format!("no human step {id}"))),
        }
    }

    pub fn human_steps(&self) -> Vec<HumanStep> {
        let mut g = self.lock();
        Self::expire_human_steps(&mut g, now_ms());
        let mut v: Vec<_> = g.human_steps.values().cloned().collect();
        v.sort_by_key(|h| h.created_at_ms);
        v
    }

    /// The person finished the step (`done`) or will not do it. Either way the screen has changed,
    /// so captures taken before it are dropped and the agent must observe again.
    pub fn resolve_human_step(&self, peer: &Peer, id: &str, done: bool) -> Result<HumanStep> {
        self.require_human(peer, "human_steps.resolve")?;
        let now = now_ms();
        let mut g = self.lock();
        Self::expire_human_steps(&mut g, now);
        let h = g.human_steps.get_mut(id).ok_or_else(|| AdlError::not_found(format!("no human step {id}")))?;
        if h.status != HumanStepStatus::Pending {
            return Err(AdlError::invalid(format!("human step is {:?}", h.status).to_lowercase()));
        }
        h.status = if done { HumanStepStatus::Done } else { HumanStepStatus::Cancelled };
        h.resolved_at_ms = Some(now);
        let info = h.clone();
        g.captures.clear();
        Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "human_step_resolved", "data": {"id": id, "status": info.status}}));
        drop(g);
        self.audit.log("human_steps.resolve", Some(&info.agent), info.task.as_deref(), if done { "approved" } else { "denied" }, "", json!({"kind": info.kind}));
        Ok(info)
    }

    // ---------------------------------------------------------------- drive prompt

    fn drive_prompt_on(&self) -> bool {
        self.cfg.drive_prompt && !self.cfg.enabled_by_default
    }

    /// Before an agent drives, the person is asked: Allow, Deny, Postpone or Always allow. One
    /// request per agent: "Allow" lasts while the agent keeps driving (see `drive_idle_ms`), so a
    /// CLI that begins a task per command is asked once, not per click. Returns whether the lease
    /// was granted by "Always allow" (no prompt).
    fn drive_gate(&self, g: &mut MutexGuard<'_, Inner>, p: &Principal, task: &str, now: u64) -> Result<bool> {
        if !self.drive_prompt_on() {
            return Ok(false);
        }
        if self.store.drive_always_allow() {
            return Ok(true);
        }
        let idle = self.cfg.drive_idle_ms;
        let existing = g.drive.values().find(|d| d.agent == p.agent).map(|d| d.id.clone());
        if let Some(id) = &existing {
            let d = g.drive.get_mut(id).unwrap();
            let until = d.until_ms.unwrap_or(0);
            match d.status {
                DriveStatus::Allowed if until > now => {
                    d.until_ms = Some(now + idle);
                    return Ok(false);
                }
                DriveStatus::Denied if until > now => {
                    return Err(AdlError::denied("the person at the desktop declined to let this agent drive").with_details(json!({"drive_request_id": id, "retry_after_ms": until - now})));
                }
                DriveStatus::Postponed if until > now => {
                    return Err(AdlError::new(ErrorCode::Busy, "the person postponed this agent's turn; try lease.acquire again later")
                        .with_details(json!({"drive_request_id": id, "retry_after_ms": until - now})));
                }
                DriveStatus::Pending if d.expires_at_ms > now => {
                    // Show what it wants to do now.
                    if let Some(t) = g.tasks.get(task) {
                        let desc = t.info.description.clone();
                        let d = g.drive.get_mut(id).unwrap();
                        d.task = task.into();
                        d.description = desc;
                    }
                    return Err(Self::drive_wait_error(id));
                }
                _ => {}
            }
        }
        // New, lapsed, idle too long, or the postponement/denial is over: ask (again).
        let agent_name = self.store.get(&p.agent).map(|a| a.name).unwrap_or_else(|| p.agent.clone());
        let description = g.tasks.get(task).map(|t| t.info.description.clone()).unwrap_or_default();
        let d = DriveRequest {
            id: existing.unwrap_or_else(|| random_id("drive", 8)),
            agent: p.agent.clone(),
            agent_name,
            task: task.into(),
            description,
            status: DriveStatus::Pending,
            created_at_ms: now,
            expires_at_ms: now + self.cfg.drive_request_ttl_ms,
            until_ms: None,
        };
        let id = d.id.clone();
        Self::broadcast(g, &json!({"v": SCHEMA_VERSION, "event": "drive_requested", "data": d}));
        g.drive.insert(id.clone(), d);
        self.audit.log("lease.acquire", Some(&p.agent), Some(task), "approval_required", "", json!({"drive_request": id}));
        Err(Self::drive_wait_error(&id))
    }

    fn drive_wait_error(id: &str) -> AdlError {
        AdlError::new(ErrorCode::ApprovalRequired, "the person at the desktop must allow this agent to drive; retry lease.acquire after they answer")
            .with_details(json!({"drive_request_id": id}))
    }

    /// An allowed agent that keeps driving keeps its allowance.
    fn drive_touch(&self, g: &mut MutexGuard<'_, Inner>, agent: &str, now: u64) {
        let idle = self.cfg.drive_idle_ms;
        if let Some(d) = g.drive.values_mut().find(|d| d.agent == agent && d.status == DriveStatus::Allowed) {
            d.until_ms = Some(now + idle);
        }
    }

    pub fn drive_requests(&self) -> Vec<DriveRequest> {
        let now = now_ms();
        let mut g = self.lock();
        for d in g.drive.values_mut() {
            if d.status == DriveStatus::Pending && d.expires_at_ms <= now {
                d.status = DriveStatus::Expired;
            }
        }
        let mut v: Vec<_> = g.drive.values().cloned().collect();
        v.sort_by_key(|d| d.created_at_ms);
        v
    }

    /// The person answers a drive prompt. `postpone_ms` only applies to Postpone.
    pub fn decide_drive(&self, peer: &Peer, id: &str, decision: DriveDecision, postpone_ms: Option<u64>) -> Result<DriveRequest> {
        self.require_human(peer, "drive.decide")?;
        let now = now_ms();
        if decision == DriveDecision::AlwaysAllow {
            self.store.set_drive_always_allow(true)?;
        }
        let mut g = self.lock();
        let d = g.drive.get_mut(id).ok_or_else(|| AdlError::not_found(format!("no drive request {id}")))?;
        if d.status != DriveStatus::Pending || d.expires_at_ms <= now {
            // "Always allow" still took effect above; only the per-request answer is refused.
            if decision != DriveDecision::AlwaysAllow {
                return Err(AdlError::invalid(format!("drive request is {:?}", d.status).to_lowercase()));
            }
        }
        match decision {
            DriveDecision::Allow | DriveDecision::AlwaysAllow => {
                d.status = DriveStatus::Allowed;
                d.until_ms = Some(now + self.cfg.drive_idle_ms);
            }
            DriveDecision::Deny => {
                d.status = DriveStatus::Denied;
                d.until_ms = Some(now + self.cfg.drive_postpone_ms);
            }
            DriveDecision::Postpone => {
                let ms = postpone_ms.unwrap_or(self.cfg.drive_postpone_ms).clamp(60_000, 4 * 3_600_000);
                d.status = DriveStatus::Postponed;
                d.until_ms = Some(now + ms);
            }
        }
        let info = d.clone();
        Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "drive_decided", "data": {"id": id, "status": info.status, "until_ms": info.until_ms}}));
        if decision == DriveDecision::AlwaysAllow {
            // Everyone else waiting gets the desktop too, without another prompt.
            let others: Vec<String> = g.drive.values().filter(|o| o.id != id && o.status == DriveStatus::Pending).map(|o| o.id.clone()).collect();
            let idle = self.cfg.drive_idle_ms;
            for o in others {
                let od = g.drive.get_mut(&o).unwrap();
                od.status = DriveStatus::Allowed;
                od.until_ms = Some(now + idle);
                Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "drive_decided", "data": {"id": o, "status": DriveStatus::Allowed}}));
            }
            Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "drive_always_allow", "data": {"on": true}}));
        }
        drop(g);
        let decision_name = serde_json::to_value(decision).unwrap();
        self.audit.log("drive.decide", Some(&info.agent), Some(&info.task), if decision == DriveDecision::Deny { "denied" } else { "approved" }, "", json!({"decision": decision_name}));
        Ok(info)
    }

    /// Turn "Always allow" on (away from the desk) or off (ask again before every agent drives).
    pub fn set_drive_always_allow(&self, peer: &Peer, on: bool) -> Result<Status> {
        self.require_human(peer, "drive.always_allow")?;
        self.store.set_drive_always_allow(on)?;
        self.broadcast_event("drive_always_allow", json!({"on": on}));
        self.audit.log("drive.always_allow", None, None, "allowed", "", json!({"on": on}));
        Ok(self.status())
    }

    pub fn lease_renew(&self, p: &Principal, ttl_ms: Option<u64>) -> Result<LeaseInfo> {
        let lease_gen = self.require_enabled()?;
        let ttl = ttl_ms.unwrap_or(DEFAULT_LEASE_TTL_MS).clamp(1_000, MAX_LEASE_TTL_MS);
        let mut g = self.lock();
        let l = g.lease.as_mut().filter(|l| l.info.agent == p.agent && l.info.generation == lease_gen && l.info.expires_at_ms > now_ms());
        let l = l.ok_or_else(|| AdlError::denied("this agent does not hold a valid lease"))?;
        l.info.expires_at_ms = now_ms() + ttl;
        let info = l.info.clone();
        self.drive_touch(&mut g, &p.agent, now_ms());
        Ok(info)
    }

    pub fn lease_release(&self, p: &Principal) -> Result<()> {
        let mut g = self.lock();
        if g.lease.as_ref().is_some_and(|l| l.info.agent == p.agent) {
            g.lease = None;
            Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "lease", "data": null}));
        }
        Ok(())
    }

    pub fn approval_status(&self, p: &Principal, id: &str) -> Result<ApprovalInfo> {
        self.approvals().into_iter().find(|a| a.id == id && a.agent == p.agent).ok_or_else(|| AdlError::not_found(format!("no approval {id}")))
    }

    // ---------------------------------------------------------------- input engine

    /// Checked before every primitive event.
    fn checkpoint(&self, p: &Principal, task: &str, gen0: u64) -> Result<()> {
        if !self.open.load(Ordering::SeqCst) || self.generation.load(Ordering::SeqCst) != gen0 {
            return Err(AdlError::new(ErrorCode::Cancelled, "access was stopped or revoked"));
        }
        let now = now_ms();
        let g = self.lock();
        match &g.lease {
            Some(l) if l.info.agent == p.agent && l.info.task == task && l.info.generation == gen0 && l.info.expires_at_ms > now => {}
            Some(l) if l.info.agent == p.agent && l.info.expires_at_ms <= now => return Err(AdlError::new(ErrorCode::Cancelled, "input lease expired")),
            _ => return Err(AdlError::new(ErrorCode::Cancelled, "input lease is not held by this task")),
        }
        if g.tasks.get(task).is_none_or(|t| t.info.state != TaskState::Running) {
            return Err(AdlError::new(ErrorCode::Cancelled, "task was cancelled"));
        }
        if g.approvals.values().any(|a| a.status == ApprovalStatus::Pending && a.expires_at_ms > now) {
            return Err(AdlError::new(ErrorCode::Busy, "input is suspended while an approval is on screen"));
        }
        if g.held.iter().any(|(_, since)| now.saturating_sub(*since) > MAX_HOLD_MS) {
            return Err(AdlError::invalid(format!("synthetic input held longer than {MAX_HOLD_MS} ms")));
        }
        Ok(())
    }

    fn emit(&self, p: &Principal, task: &str, gen0: u64, ev: InputEvent) -> Result<()> {
        self.checkpoint(p, task, gen0)?;
        self.adapter.emit(&ev)?;
        let mut g = self.lock();
        match &ev {
            InputEvent::Button { button, pressed: true } => g.held.push((Held::Button(*button), now_ms())),
            InputEvent::Key { key, pressed: true } => g.held.push((Held::Key(key.clone()), now_ms())),
            InputEvent::Button { button, pressed: false } => g.held.retain(|(h, _)| !matches!(h, Held::Button(b) if b == button)),
            InputEvent::Key { key, pressed: false } => g.held.retain(|(h, _)| !matches!(h, Held::Key(k) if k == key)),
            _ => {}
        }
        Ok(())
    }

    fn sleep(&self, p: &Principal, task: &str, gen0: u64, ms: u64) -> Result<()> {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            self.checkpoint(p, task, gen0)?;
            std::thread::sleep(Duration::from_millis(10).min(end.saturating_duration_since(Instant::now())));
        }
        Ok(())
    }

    fn pace(&self, p: &Principal, task: &str, gen0: u64) -> Result<()> {
        if self.cfg.pace_ms > 0 {
            self.sleep(p, task, gen0, self.cfg.pace_ms)?;
        }
        Ok(())
    }

    /// Map a point to desktop logical coordinates and validate what is under it.
    fn resolve(&self, pt: &Point, layout: &Layout, ctx: &BatchCtx, check_target: bool) -> Result<(f64, f64)> {
        let (x, y) = match &pt.space {
            Space::Desktop => (pt.x, pt.y),
            Space::Display { display } => {
                let d = layout.displays.iter().find(|d| &d.id == display).ok_or_else(|| AdlError::stale(format!("display {display} is not connected")))?;
                if pt.x < 0.0 || pt.y < 0.0 || pt.x >= d.logical.width || pt.y >= d.logical.height {
                    return Err(AdlError::invalid(format!("point is outside display {display}")));
                }
                (d.logical.x + pt.x, d.logical.y + pt.y)
            }
            Space::Window { window } => {
                let w = self.adapter.windows()?.into_iter().find(|w| &w.id == window).ok_or_else(|| AdlError::stale(format!("window {window} no longer exists")))?;
                let b = w.bounds.ok_or_else(|| AdlError::unsupported("this backend does not report window geometry"))?;
                if pt.x < 0.0 || pt.y < 0.0 || pt.x >= b.width || pt.y >= b.height {
                    return Err(AdlError::invalid(format!("point is outside window {window}")));
                }
                (b.x + pt.x, b.y + pt.y)
            }
            Space::Capture { capture } => {
                let c = self.lock().captures.get(capture).cloned().ok_or_else(|| AdlError::stale(format!("capture {capture} is unknown or expired")))?;
                if c.expires_at_ms <= now_ms() {
                    return Err(AdlError::stale("capture has expired; take a new screenshot"));
                }
                if c.layout_generation != layout.generation {
                    return Err(AdlError::stale("display layout changed since the screenshot"));
                }
                c.to_desktop(pt.x, pt.y).ok_or_else(|| AdlError::invalid("point is outside the screenshot"))?
            }
        };
        if !layout.displays.iter().any(|d| d.logical.contains(x, y)) {
            return Err(AdlError::invalid(format!("({x}, {y}) is not on any display")));
        }
        let hit = self.adapter.window_at(x, y)?;
        if check_target
            && let Some(t) = &ctx.target_window {
                match &hit {
                    Some(h) if &h.id == t => {}
                    Some(h) => return Err(AdlError::stale(format!("target is covered by {} ({})", h.title, h.app_id)).with_details(json!({"covering_window": h.id}))),
                    None => return Err(AdlError::stale("the point is not over the target window")),
                }
            }
        if ctx.policy.mode == ControlMode::Restricted {
            match &hit {
                Some(h) if ctx.policy.allows_app(&h.app_id) => {}
                Some(h) => return Err(AdlError::denied(format!("{} is not in this agent's application grant", h.app_id))),
                None => return Err(AdlError::denied("restricted agents may only act on granted windows")),
            }
        }
        Ok((x, y))
    }

    fn check_focus(&self, ctx: &BatchCtx) -> Result<()> {
        if let Some(t) = &ctx.target_window {
            let f = self.adapter.focused_window()?;
            if f.as_ref().map(|w| &w.id) != Some(t) {
                let now = f.map(|w| format!("{} ({})", w.title, w.app_id)).unwrap_or_else(|| "nothing".into());
                return Err(AdlError::stale(format!("focus moved away from the target window to {now}")));
            }
        }
        Ok(())
    }

    fn modifiers_down(&self, p: &Principal, ctx: &BatchCtx, mods: &[Modifier]) -> Result<()> {
        for m in mods {
            self.emit(p, &ctx.task, ctx.gen0, InputEvent::Key { key: m.key(), pressed: true })?;
        }
        Ok(())
    }

    fn modifiers_up(&self, p: &Principal, ctx: &BatchCtx, mods: &[Modifier]) -> Result<()> {
        for m in mods.iter().rev() {
            self.emit(p, &ctx.task, ctx.gen0, InputEvent::Key { key: m.key(), pressed: false })?;
        }
        Ok(())
    }

    fn run_action(&self, p: &Principal, ctx: &BatchCtx, a: &Action) -> std::result::Result<(), Fail> {
        if let Some(why) = self.adapter.safety_interlock() {
            return Err(Fail::Before(AdlError::new(ErrorCode::Blocked, format!("input paused: {why}"))));
        }
        let layout = self.adapter.layout().map_err(Fail::Before)?;
        let (task, gen0) = (ctx.task.as_str(), ctx.gen0);
        let ev = |e| self.emit(p, task, gen0, e).map_err(Fail::During);
        let before = |r: Result<()>| r.map_err(Fail::Before);
        let broad = ctx.policy.mode == ControlMode::BroadDesktop;
        if a.is_keyboard() {
            before(self.check_focus(ctx))?;
        }
        match a {
            Action::Move { to } => {
                let (x, y) = self.resolve(to, &layout, ctx, false).map_err(Fail::Before)?;
                ev(InputEvent::MotionAbsolute { x, y })?;
            }
            Action::MoveRelative { dx, dy } => {
                if !broad {
                    return Err(Fail::Before(AdlError::denied("relative motion has no target; it needs broad_desktop control")));
                }
                ev(InputEvent::MotionRelative { dx: *dx, dy: *dy })?;
            }
            Action::Click { button, count, at, modifiers } => {
                match at {
                    Some(pt) => {
                        let (x, y) = self.resolve(pt, &layout, ctx, true).map_err(Fail::Before)?;
                        ev(InputEvent::MotionAbsolute { x, y })?;
                    }
                    None => {
                        let pos = self.adapter.pointer_position().map_err(Fail::Before)?;
                        match pos {
                            Some((x, y)) => drop(self.resolve(&Point::desktop(x, y), &layout, ctx, true).map_err(Fail::Before)?),
                            None if broad && ctx.target_window.is_none() => {}
                            None => return Err(Fail::Before(AdlError::invalid("pass `at`: this backend cannot report the pointer position to validate the target"))),
                        }
                    }
                }
                self.modifiers_down(p, ctx, modifiers).map_err(Fail::During)?;
                for i in 0..*count {
                    if i > 0 {
                        self.pace(p, task, gen0).map_err(Fail::During)?;
                    }
                    ev(InputEvent::Button { button: *button, pressed: true })?;
                    ev(InputEvent::Button { button: *button, pressed: false })?;
                }
                self.modifiers_up(p, ctx, modifiers).map_err(Fail::During)?;
            }
            Action::ButtonDown { button } => ev(InputEvent::Button { button: *button, pressed: true })?,
            Action::ButtonUp { button } => ev(InputEvent::Button { button: *button, pressed: false })?,
            Action::Drag { from, to, button, steps } => {
                let (x0, y0) = self.resolve(from, &layout, ctx, true).map_err(Fail::Before)?;
                let (x1, y1) = self.resolve(to, &layout, ctx, false).map_err(Fail::Before)?;
                ev(InputEvent::MotionAbsolute { x: x0, y: y0 })?;
                ev(InputEvent::Button { button: *button, pressed: true })?;
                for i in 1..=*steps {
                    self.pace(p, task, gen0).map_err(Fail::During)?;
                    let t = i as f64 / *steps as f64;
                    ev(InputEvent::MotionAbsolute { x: x0 + (x1 - x0) * t, y: y0 + (y1 - y0) * t })?;
                }
                ev(InputEvent::Button { button: *button, pressed: false })?;
            }
            Action::Scroll { axis, steps, at } => {
                if let Some(pt) = at {
                    let (x, y) = self.resolve(pt, &layout, ctx, true).map_err(Fail::Before)?;
                    ev(InputEvent::MotionAbsolute { x, y })?;
                } else if !broad {
                    return Err(Fail::Before(AdlError::denied("restricted agents must scroll `at` a granted window")));
                }
                let unit = steps.signum();
                for i in 0..steps.abs() {
                    if i > 0 {
                        self.pace(p, task, gen0).map_err(Fail::During)?;
                    }
                    ev(InputEvent::Scroll { axis: *axis, steps: unit })?;
                }
            }
            Action::Hover { to, ms } => {
                let (x, y) = self.resolve(to, &layout, ctx, false).map_err(Fail::Before)?;
                ev(InputEvent::MotionAbsolute { x, y })?;
                self.sleep(p, task, gen0, *ms).map_err(Fail::During)?;
            }
            Action::Type { text } => {
                let chars: Vec<char> = text.chars().collect();
                for (i, chunk) in chars.chunks(TEXT_CHUNK_CHARS).enumerate() {
                    if i > 0 {
                        self.check_focus(ctx).map_err(Fail::During)?;
                    }
                    ev(InputEvent::Text { text: chunk.iter().collect() })?;
                }
            }
            Action::KeyPress { key } => {
                ev(InputEvent::Key { key: key.clone(), pressed: true })?;
                ev(InputEvent::Key { key: key.clone(), pressed: false })?;
            }
            Action::KeyDown { key } => ev(InputEvent::Key { key: key.clone(), pressed: true })?,
            Action::KeyUp { key } => ev(InputEvent::Key { key: key.clone(), pressed: false })?,
            Action::Shortcut { keys } => {
                self.modifiers_down(p, ctx, &keys.modifiers).map_err(Fail::During)?;
                ev(InputEvent::Key { key: keys.key.clone(), pressed: true })?;
                ev(InputEvent::Key { key: keys.key.clone(), pressed: false })?;
                self.modifiers_up(p, ctx, &keys.modifiers).map_err(Fail::During)?;
            }
            Action::Wait { ms } => self.sleep(p, task, gen0, *ms).map_err(Fail::During)?,
        }
        Ok(())
    }

    /// Human-readable approval summary. Typed text is reported by length only.
    fn summarize(batch: &InputBatch, target_app: Option<&str>) -> String {
        let parts: Vec<String> = batch
            .actions
            .iter()
            .map(|a| match a {
                Action::Type { text } => format!("type {} characters", text.chars().count()),
                Action::Shortcut { keys } => format!("shortcut {keys}"),
                Action::KeyPress { key } => format!("press {key}"),
                Action::Click { button, count, .. } => format!("{} {:?} click", if *count > 1 { format!("{count}x") } else { String::new() }, button).trim().to_lowercase(),
                other => other.kind().replace('_', " "),
            })
            .collect();
        match target_app {
            Some(app) => format!("{} in {app}", parts.join(", ")),
            None => parts.join(", "),
        }
    }

    fn check_approval(&self, p: &Principal, pol: &Policy, batch: &InputBatch, target_app: Option<&str>) -> Result<()> {
        let needs = pol.approval_required_for.contains("input") || batch.actions.iter().any(|a| pol.approval_required_for.contains(a.kind()));
        if !needs {
            return Ok(());
        }
        let canonical = serde_json::to_vec(&json!({"agent": p.agent, "task": batch.task, "target": batch.target, "actions": batch.actions})).unwrap();
        let digest = sha256_hex(&canonical);
        let now = now_ms();
        let mut g = self.lock();
        if let Some(a) = g.approvals.values_mut().find(|a| a.agent == p.agent && a.args_digest == digest && a.status == ApprovalStatus::Approved && a.expires_at_ms > now) {
            a.status = ApprovalStatus::Consumed;
            return Ok(());
        }
        let existing = g.approvals.values().find(|a| a.agent == p.agent && a.args_digest == digest && a.status == ApprovalStatus::Pending && a.expires_at_ms > now).map(|a| a.id.clone());
        let id = match existing {
            Some(id) => id,
            None => {
                let a = ApprovalInfo {
                    id: random_id("appr", 8), agent: p.agent.clone(), task: batch.task.clone(), operation: "input".into(),
                    target: target_app.unwrap_or("desktop").into(), args_digest: digest, summary: Self::summarize(batch, target_app),
                    status: ApprovalStatus::Pending, created_at_ms: now, expires_at_ms: now + self.cfg.approval_ttl_ms,
                };
                let id = a.id.clone();
                Self::broadcast(&mut g, &json!({"v": SCHEMA_VERSION, "event": "approval_requested", "data": a}));
                g.approvals.insert(id.clone(), a);
                id
            }
        };
        drop(g);
        self.audit.log("input", Some(&p.agent), Some(&batch.task), "approval_required", "", json!({"approval": id}));
        Err(AdlError::new(ErrorCode::ApprovalRequired, "a human must approve this input; retry the identical request after approval").with_details(json!({"approval_id": id})))
    }

    fn check_rate(&self, p: &Principal, pol: &Policy, n: usize) -> Result<()> {
        let limit = if pol.max_actions_per_minute == 0 { self.cfg.default_actions_per_minute } else { pol.max_actions_per_minute } as usize;
        let now = now_ms();
        let mut g = self.lock();
        let q = g.rate.entry(p.agent.clone()).or_default();
        while q.front().is_some_and(|t| now - t > 60_000) {
            q.pop_front();
        }
        if q.len() + n > limit {
            return Err(AdlError::new(ErrorCode::Busy, format!("action limit of {limit} per minute reached")));
        }
        q.extend(std::iter::repeat_n(now, n));
        Ok(())
    }

    /// Execute one bounded input batch.
    pub fn input(&self, p: &Principal, batch: &InputBatch) -> Result<BatchResult> {
        batch.validate()?;
        let gen0 = self.require_enabled()?;
        let pol = self.policy(p)?;
        if batch.actions.iter().any(Action::is_pointer) && !pol.capabilities.contains(&Capability::Pointer) {
            return Err(AdlError::denied("agent is not granted pointer"));
        }
        if batch.actions.iter().any(Action::is_keyboard) && !pol.capabilities.contains(&Capability::Keyboard) {
            return Err(AdlError::denied("agent is not granted keyboard"));
        }
        let mut target_app = None;
        if let Some(w) = &batch.target.window {
            let win = self.adapter.windows()?.into_iter().find(|x| &x.id == w).ok_or_else(|| AdlError::stale(format!("window {w} no longer exists")))?;
            if !pol.allows_app(&win.app_id) {
                return Err(AdlError::denied(format!("{} is not in this agent's application grant", win.app_id)));
            }
            target_app = Some(win.app_id);
        } else if pol.mode != ControlMode::BroadDesktop && batch.actions.iter().any(Action::is_keyboard) {
            return Err(AdlError::denied("restricted agents must name a target window for keyboard input"));
        }
        if let Some(lg) = batch.target.layout_generation
            && lg != self.adapter.layout()?.generation {
                return Err(AdlError::stale("display layout changed; observe again"));
            }
        // Before approval and before any event: a batch the backend cannot deliver exactly is
        // refused whole, never typed in part.
        for a in &batch.actions {
            self.adapter.preflight(a)?;
        }
        self.checkpoint(p, &batch.task, gen0)?;
        self.check_approval(p, &pol, batch, target_app.as_deref())?;
        self.check_rate(p, &pol, batch.actions.len())?;

        let _dispatch = match self.dispatch.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Err(AdlError::new(ErrorCode::Busy, "another input batch is being delivered")),
        };
        let ctx = BatchCtx { task: batch.task.clone(), gen0, policy: pol, target_window: batch.target.window.clone() };
        let request = random_id("req", 8);
        let mut done = 0;
        let mut failure = None;
        for a in &batch.actions {
            match self.run_action(p, &ctx, a) {
                Ok(()) => done += 1,
                Err(f) => {
                    failure = Some(f);
                    break;
                }
            }
        }
        // Bounded holds: nothing this layer pressed outlives the batch.
        let mut cleanup = vec![];
        let released = self.release_held(&mut cleanup);

        let (outcome, error) = match failure {
            None => (Outcome::Completed, None),
            Some(Fail::Before(e)) | Some(Fail::During(e)) if e.code == ErrorCode::Cancelled => (Outcome::Cancelled, Some(e)),
            Some(Fail::Before(e)) => (Outcome::Failed, Some(e)),
            Some(Fail::During(e)) if matches!(e.code, ErrorCode::TargetStale | ErrorCode::Busy | ErrorCode::Invalid) => (Outcome::Failed, Some(e)),
            Some(Fail::During(e)) => (Outcome::OutcomeUnknown, Some(AdlError::new(ErrorCode::OutcomeUnknown, e.message.clone()).with_details(json!({"cause": e.code})))),
        };
        {
            let mut g = self.lock();
            if let Some(t) = g.tasks.get_mut(&batch.task) {
                match outcome {
                    Outcome::Completed => t.info.batches_completed += 1,
                    Outcome::Cancelled => t.info.batches_cancelled += 1,
                    Outcome::OutcomeUnknown => t.info.batches_unknown += 1,
                    Outcome::Failed => {}
                }
            }
        }
        let kinds: Vec<&str> = batch.actions.iter().map(Action::kind).collect();
        self.audit.log(
            "input",
            Some(&p.agent),
            Some(&batch.task),
            "allowed",
            serde_json::to_value(outcome).unwrap().as_str().unwrap_or_default(),
            json!({"kinds": kinds, "completed": done, "total": batch.actions.len(), "app": target_app, "released": released, "error": error.as_ref().map(|e| e.code)}),
        );
        Ok(BatchResult {
            request,
            task: batch.task.clone(),
            outcome,
            verification: if outcome == Outcome::Completed { Verification::Dispatched } else { Verification::NotVerified },
            backend: self.adapter.name(),
            actions_completed: done,
            actions_total: batch.actions.len(),
            error,
        })
    }
}

struct BatchCtx {
    task: String,
    gen0: u64,
    policy: Policy,
    target_window: Option<String>,
}
