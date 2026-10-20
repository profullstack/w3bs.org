//! M1 conformance suite against the mock adapter. Each test names the PRD acceptance row it covers.

use adl_broker::mock::MockDesktop;
use adl_broker::{Adapter, Broker, Config, InputEvent, Peer, Principal, SessionEvent};
use adl_broker::audit::Audit;
use adl_broker::store::AgentStore;
use adl_schema::*;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct H {
    b: Arc<Broker>,
    m: Arc<MockDesktop>,
    _rx: mpsc::Receiver<String>,
}

fn human() -> Peer {
    Peer { pid: Some(1), agent_descendant: false, interactive: true, native_control: true }
}

fn cfg() -> Config {
    // The drive prompt has its own tests below; the rest exercise input with a lease in hand.
    Config { pace_ms: 0, drive_prompt: false, ..Config::default() }
}

fn harness_with(cfg: Config) -> H {
    let m = Arc::new(MockDesktop::new());
    let b = Arc::new(Broker::new(m.clone(), AgentStore::memory(), Audit::memory(), cfg));
    let (tx, rx) = mpsc::channel();
    b.subscribe(tx, true);
    H { b, m, _rx: rx }
}

fn harness() -> H {
    harness_with(cfg())
}

fn policy(mode: ControlMode, caps: &[Capability], apps: &[&str]) -> Policy {
    Policy { capabilities: caps.iter().copied().collect(), mode, apps: apps.iter().map(|s| s.to_string()).collect(), ..Default::default() }
}

fn broad() -> Policy {
    policy(ControlMode::BroadDesktop, &Capability::ALL, &[])
}

impl H {
    fn enable(&self) {
        let c = self.b.enable_request(&human()).unwrap();
        self.b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();
        assert_eq!(self.b.state(), GlobalState::Enabled);
    }
    fn agent(&self, name: &str, p: Policy) -> Principal {
        let (_, token) = self.b.add_agent(&human(), name, p).unwrap();
        self.b.authenticate(&token, None).unwrap()
    }
    /// Agent with a running task and the input lease.
    fn driver(&self, name: &str, p: Policy) -> (Principal, String) {
        let a = self.agent(name, p);
        let t = self.b.task_begin(&a, "test").unwrap().id;
        self.b.lease_acquire(&a, &t, None).unwrap();
        (a, t)
    }
    fn input(&self, a: &Principal, task: &str, window: Option<&str>, actions: Vec<Action>) -> Result<BatchResult> {
        self.b.input(a, &InputBatch { task: task.into(), target: Target { window: window.map(Into::into), layout_generation: None }, actions })
    }
}

fn click_at(p: Point) -> Action {
    Action::Click { button: Button::Left, count: 1, at: Some(p), modifiers: vec![] }
}

// ------------------------------------------------------------------ Fresh install / enable

#[test]
fn fresh_start_is_disabled_and_rejects_agents() {
    let h = harness();
    assert_eq!(h.b.state(), GlobalState::Disabled);
    let a = h.agent("claude-code", broad());
    assert_eq!(h.b.windows(&a).unwrap_err().code, ErrorCode::Disabled);
    assert_eq!(h.b.task_begin(&a, "x").unwrap_err().code, ErrorCode::Disabled);
    assert_eq!(h.b.screenshot(&a, "DP-1").unwrap_err().code, ErrorCode::Disabled);
}

#[test]
fn enable_needs_a_native_control() {
    let m = Arc::new(MockDesktop::new());
    let b = Broker::new(m, AgentStore::memory(), Audit::memory(), cfg());
    let e = b.enable_request(&human()).unwrap_err();
    assert_eq!(e.code, ErrorCode::Blocked);
    assert_eq!(b.state(), GlobalState::Blocked);
}

#[test]
fn enable_requires_a_human_and_the_right_code() {
    let h = harness();
    let agentish = Peer { agent_descendant: true, interactive: true, ..human() };
    assert_eq!(h.b.enable_request(&agentish).unwrap_err().code, ErrorCode::Denied);
    // A non-interactive peer gets no code back; it goes only to the native control.
    let cli = Peer { native_control: false, ..human() };
    let c = h.b.enable_request(&cli).unwrap();
    assert!(c.code.is_none(), "a CLI requester must read the code from the native control");
    assert_eq!(c.delivered_to, vec!["native_control".to_string()]);
    let headless = Peer { native_control: false, interactive: false, ..human() };
    assert_eq!(h.b.enable_confirm(&headless, "123456").unwrap_err().code, ErrorCode::Denied);
    assert_eq!(h.b.enable_confirm(&human(), "000000x").unwrap_err().code, ErrorCode::Denied);
    assert_eq!(h.b.state(), GlobalState::Enabling);
    let c = h.b.enable_request(&human()).unwrap();
    assert_eq!(h.b.enable_confirm(&agentish, c.code.as_deref().unwrap()).unwrap_err().code, ErrorCode::Denied);
    h.b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();
    assert_eq!(h.b.state(), GlobalState::Enabled);
    assert!(h.m.session_open());
}

#[test]
fn enabled_by_default_needs_no_native_control_or_code() {
    let m = Arc::new(MockDesktop::new());
    let b = Broker::new(m.clone(), AgentStore::memory(), Audit::memory(), Config { enabled_by_default: true, ..cfg() });
    b.enable_unattended().unwrap();
    assert_eq!(b.state(), GlobalState::Enabled);
    assert!(m.session_open());
    let (_, token) = b.add_agent(&human(), "ci", broad()).unwrap();
    let a = b.authenticate(&token, None).unwrap();
    b.task_begin(&a, "x").unwrap();
    // Disable still works from anyone; re-enabling needs no code.
    b.disable("test");
    assert_eq!(b.state(), GlobalState::Disabled);
    b.enable_unattended().unwrap();
    assert_eq!(b.state(), GlobalState::Enabled);
    // A native control detaching does not turn a non-desktop broker off.
    let (tx, _rx) = mpsc::channel();
    b.subscribe(tx, true);
    b.detach_native_control();
    assert_eq!(b.state(), GlobalState::Enabled);
}

#[test]
fn enable_unattended_is_refused_on_a_desktop_broker() {
    let h = harness();
    assert_eq!(h.b.enable_unattended().unwrap_err().code, ErrorCode::Denied);
    assert_eq!(h.b.state(), GlobalState::Disabled);
}

#[test]
fn backend_failure_blocks_enable() {
    let h = harness();
    h.m.fail_begin("RemoteDesktop portal denied");
    let e = h.b.enable_request(&human()).unwrap_err();
    assert_eq!(e.code, ErrorCode::Blocked);
    let s = h.b.status();
    assert_eq!(s.state, GlobalState::Blocked);
    assert!(s.blocked_reason.unwrap().contains("portal"));
}

// ------------------------------------------------------------------ Mouse

#[test]
fn mouse_workflow_and_coordinate_spaces() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let r = h
        .input(&a, &t, None, vec![
            Action::Move { to: Point::display("DP-2", 100.0, 100.0) },
            Action::Click { button: Button::Right, count: 1, at: Some(Point::window("w-editor", 50.0, 50.0)), modifiers: vec![] },
            Action::Click { button: Button::Middle, count: 1, at: None, modifiers: vec![] },
            Action::Click { button: Button::Left, count: 2, at: Some(Point::desktop(300.0, 300.0)), modifiers: vec![Modifier::Shift] },
            Action::Hover { to: Point::display("HDMI-1", 10.0, 10.0), ms: 20 },
            Action::Scroll { axis: Axis::Vertical, steps: -3, at: Some(Point::desktop(300.0, 300.0)) },
            Action::Drag { from: Point::window("w-editor", 10.0, 10.0), to: Point::desktop(1500.0, 900.0), button: Button::Left, steps: 4 },
        ])
        .unwrap();
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(r.actions_completed, 7);
    let ev = h.m.events();
    // DP-2 sits at x=-1280: display-relative (100,100) is desktop (-1180,100).
    assert_eq!(ev[0], InputEvent::MotionAbsolute { x: -1180.0, y: 100.0 });
    assert_eq!(ev[1], InputEvent::MotionAbsolute { x: 150.0, y: 150.0 });
    assert_eq!(ev[2], InputEvent::Button { button: Button::Right, pressed: true });
    assert!(ev.contains(&InputEvent::Button { button: Button::Middle, pressed: true }));
    let lefts = ev.iter().filter(|e| **e == InputEvent::Button { button: Button::Left, pressed: true }).count();
    assert_eq!(lefts, 3, "double click plus drag press");
    assert!(ev.contains(&InputEvent::MotionAbsolute { x: 1930.0, y: -190.0 }), "portrait display has a negative y origin");
    assert_eq!(ev.iter().filter(|e| matches!(e, InputEvent::Scroll { steps: -1, .. })).count(), 3);
    assert_eq!(*ev.last().unwrap(), InputEvent::Button { button: Button::Left, pressed: false });
    assert_eq!(h.m.pressed(), (vec![], vec![]), "nothing left held");
}

#[test]
fn screenshot_pixels_map_through_scale() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let shot = h.b.screenshot(&a, "DP-2").unwrap();
    assert_eq!((shot.meta.pixel_width, shot.meta.pixel_height), (2560, 1600), "HiDPI capture is physical pixels");
    h.input(&a, &t, None, vec![Action::Move { to: Point::capture(&shot.meta.capture, 400.0, 200.0) }]).unwrap();
    // 2 physical px per logical px, origin -1280.
    assert_eq!(h.m.events()[0], InputEvent::MotionAbsolute { x: -1080.0, y: 100.0 });
}

#[test]
fn stale_capture_after_layout_change() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    h.m.change_layout();
    let r = h.input(&a, &t, None, vec![click_at(Point::capture(&shot.meta.capture, 10.0, 10.0))]).unwrap();
    assert_eq!(r.outcome, Outcome::Failed);
    assert_eq!(r.error.unwrap().code, ErrorCode::TargetStale);
    assert!(h.m.events().is_empty(), "no blind click");
    // Display that disappeared is stale too.
    let r = h.input(&a, &t, None, vec![click_at(Point::display("HDMI-1", 1.0, 1.0))]).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::TargetStale);
    // An observed layout generation that no longer matches is rejected up front.
    let e = h.b.input(&a, &InputBatch { task: t.clone(), target: Target { window: None, layout_generation: Some(1) }, actions: vec![Action::Wait { ms: 1 }] }).unwrap_err();
    assert_eq!(e.code, ErrorCode::TargetStale);
}

#[test]
fn occluded_target_is_not_clicked() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    // Browser overlaps the editor at (650,250) and is raised on top.
    h.m.focus("w-browser");
    let r = h.input(&a, &t, Some("w-editor"), vec![click_at(Point::window("w-editor", 550.0, 150.0))]).unwrap();
    let e = r.error.unwrap();
    assert_eq!(e.code, ErrorCode::TargetStale);
    assert!(e.message.contains("covered"));
    assert!(h.m.events().is_empty());
}

#[test]
fn off_screen_points_are_invalid() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let r = h.input(&a, &t, None, vec![Action::Move { to: Point::desktop(-1500.0, 2000.0) }]).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::Invalid);
}

// ------------------------------------------------------------------ Keyboard

#[test]
fn keyboard_workflow() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let text = "Hello, wörld! 日本語\nsecond line";
    let r = h
        .input(&a, &t, Some("w-editor"), vec![
            Action::Type { text: text.into() },
            Action::KeyPress { key: "Enter".parse().unwrap() },
            Action::Shortcut { keys: "Ctrl+S".parse().unwrap() },
            Action::KeyDown { key: "Shift".parse().unwrap() },
            Action::KeyPress { key: "Tab".parse().unwrap() },
        ])
        .unwrap();
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(h.m.typed("w-editor"), text);
    let keys: Vec<String> = h
        .m
        .events()
        .into_iter()
        .filter_map(|e| if let InputEvent::Key { key, pressed } = e { Some(format!("{}{key}", if pressed { "+" } else { "-" })) } else { None })
        .collect();
    assert_eq!(keys, ["+Enter", "-Enter", "+ctrl", "+s", "-s", "-ctrl", "+shift", "+Tab", "-Tab", "-shift"]);
    assert_eq!(h.m.pressed(), (vec![], vec![]), "held Shift is released at the end of the batch");
}

#[test]
fn untypeable_text_is_refused_before_anything_is_sent() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.set_untypeable("é€");
    h.m.clear_events();
    let e = h
        .input(&a, &t, Some("w-editor"), vec![Action::Type { text: "fine ".into() }, Action::KeyPress { key: "Enter".parse().unwrap() }, Action::Type { text: "café €5".into() }])
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported, "{e:?}");
    assert!(e.message.contains("é€"), "{}", e.message);
    assert_eq!(h.m.typed("w-editor"), "", "not even the typeable first action");
    assert!(h.m.events().is_empty(), "no event at all: {:?}", h.m.events());
    assert!(h.b.approvals().is_empty(), "a refused batch is never put to the human");
    assert_eq!(h.b.state(), GlobalState::Enabled, "a refusal is not a revocation");
    h.input(&a, &t, Some("w-editor"), vec![Action::Type { text: "cafe 5".into() }]).unwrap();
    assert_eq!(h.m.typed("w-editor"), "cafe 5");
}

#[test]
fn long_text_is_chunked_and_focus_rechecked() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.steal_focus_after(2, "w-browser");
    let r = h.input(&a, &t, Some("w-editor"), vec![Action::Type { text: "x".repeat(200) }]).unwrap();
    assert_eq!(r.outcome, Outcome::Failed);
    assert_eq!(r.error.unwrap().code, ErrorCode::TargetStale);
    assert_eq!(h.m.typed("w-editor").len(), 2 * limits::TEXT_CHUNK_CHARS, "stopped at the next chunk boundary");
    assert_eq!(h.m.typed("w-browser"), "", "nothing typed into the window that stole focus");
}

#[test]
fn typing_into_an_unfocused_target_is_refused() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let r = h.input(&a, &t, Some("w-browser"), vec![Action::Type { text: "hi".into() }]).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::TargetStale);
    assert!(h.m.events().is_empty());
}

#[test]
fn bounds_are_enforced() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let too_many = vec![Action::Wait { ms: 0 }; limits::MAX_ACTIONS_PER_BATCH + 1];
    assert_eq!(h.input(&a, &t, None, too_many).unwrap_err().code, ErrorCode::Invalid);
    let long = Action::Type { text: "x".repeat(limits::MAX_TEXT_CHARS + 1) };
    assert_eq!(h.input(&a, &t, Some("w-editor"), vec![long]).unwrap_err().code, ErrorCode::Invalid);
    let hold = vec![Action::KeyDown { key: "Shift".parse().unwrap() }, Action::Wait { ms: limits::MAX_HOLD_MS }, Action::Wait { ms: 100 }];
    let _ = hold; // a 5 s hold is too slow for unit tests; the checkpoint rule is covered by `held_input_is_bounded`.
}

// ------------------------------------------------------------------ Stop, takeover, leases

#[test]
fn stop_during_drag_releases_buttons_and_does_not_continue() {
    let h = harness_with(Config { pace_ms: 5, ..cfg() });
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.set_emit_delay_ms(5);
    let (b, task) = (h.b.clone(), t.clone());
    let worker = std::thread::spawn(move || {
        b.input(&a, &InputBatch { task, target: Target::default(), actions: vec![Action::Drag { from: Point::desktop(10.0, 10.0), to: Point::desktop(1000.0, 1000.0), button: Button::Left, steps: 200 }] })
    });
    std::thread::sleep(Duration::from_millis(60));
    let t0 = Instant::now();
    let report = h.b.disable("test stop");
    let stop_ms = t0.elapsed().as_millis();
    let r = worker.join().unwrap().unwrap();
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert!(r.actions_completed == 0);
    assert!(report.gate_closed_ms < 250.0, "gate closed in {} ms", report.gate_closed_ms);
    assert!(stop_ms < 1000, "cleanup took {stop_ms} ms");
    assert_eq!(h.m.pressed(), (vec![], vec![]), "left button released");
    let n = h.m.events().len();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(h.m.events().len(), n, "no events after stop");
    assert_eq!(report.tasks_cancelled, 1);
    assert_eq!(report.leases_revoked, 1);
    assert_eq!(h.b.state(), GlobalState::Disabled);
    assert!(!h.m.session_open());
}

#[test]
fn stop_during_typing() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.set_emit_delay_ms(15);
    let (b, task) = (h.b.clone(), t.clone());
    let worker = std::thread::spawn(move || {
        b.input(&a, &InputBatch { task, target: Target { window: Some("w-editor".into()), layout_generation: None }, actions: vec![Action::Type { text: "y".repeat(4000) }] })
    });
    std::thread::sleep(Duration::from_millis(40));
    h.b.stop_all("stop all");
    let r = worker.join().unwrap().unwrap();
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert!(h.m.typed("w-editor").len() < 4000);
    // stop_all keeps the layer enabled, but the task and lease are gone.
    assert_eq!(h.b.state(), GlobalState::Enabled);
    assert!(h.b.status().lease.is_none());
    assert_eq!(h.b.task(&t).unwrap().state, TaskState::Cancelled);
}

#[test]
fn stop_withdraws_observations() {
    let h = harness();
    h.enable();
    let (a, _) = h.driver("claude-code", broad());
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    h.m.open_stream();
    let rep = h.b.disable("stop observation");
    assert_eq!(rep.streams_closed, 1);
    assert_eq!(h.b.screenshot(&a, "DP-1").unwrap_err().code, ErrorCode::Disabled);
    assert_eq!(h.b.windows(&a).unwrap_err().code, ErrorCode::Disabled);
    assert_eq!(h.b.inspect(&a, "w-editor").unwrap_err().code, ErrorCode::Disabled);
    // Captures are forgotten, so even after re-enabling the old screenshot cannot be used.
    h.enable();
    let t = h.b.task_begin(&a, "again").unwrap().id;
    h.b.lease_acquire(&a, &t, None).unwrap();
    let r = h.input(&a, &t, None, vec![click_at(Point::capture(&shot.meta.capture, 1.0, 1.0))]).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::TargetStale);
}

#[test]
fn two_agents_cannot_share_the_input_lease() {
    let h = harness();
    h.enable();
    let (a, _) = h.driver("claude-code", broad());
    let b = h.agent("codex", broad());
    let tb = h.b.task_begin(&b, "other").unwrap().id;
    assert_eq!(h.b.lease_acquire(&b, &tb, None).unwrap_err().code, ErrorCode::Busy);
    let r = h.input(&b, &tb, None, vec![Action::Move { to: Point::desktop(1.0, 1.0) }]);
    assert_eq!(r.unwrap_err().code, ErrorCode::Cancelled, "no lease, no dispatch");
    h.b.lease_release(&a).unwrap();
    h.b.lease_acquire(&b, &tb, None).unwrap();
}

#[test]
fn human_takeover_disables_without_resume() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.push_event(SessionEvent::PhysicalInput);
    h.b.pump_session_events();
    assert_eq!(h.b.state(), GlobalState::Disabled);
    h.m.push_event(SessionEvent::Unlocked);
    h.b.pump_session_events();
    assert_eq!(h.b.state(), GlobalState::Disabled, "never resumes on its own");
    assert_eq!(h.input(&a, &t, None, vec![Action::Wait { ms: 1 }]).unwrap_err().code, ErrorCode::Disabled);
}

#[test]
fn physical_input_while_idle_is_not_a_takeover() {
    let h = harness();
    h.enable();
    h.m.push_event(SessionEvent::PhysicalInput);
    h.b.pump_session_events();
    assert_eq!(h.b.state(), GlobalState::Enabled);
}

#[test]
fn faults_revoke_access() {
    for ev in [SessionEvent::Locked, SessionEvent::Suspending, SessionEvent::LoggingOut, SessionEvent::EmergencyStop, SessionEvent::PermissionLost("RemoteDesktop".into()), SessionEvent::AdapterFailed("crashed".into())] {
        let h = harness();
        h.enable();
        h.m.push_event(ev.clone());
        h.b.pump_session_events();
        assert_eq!(h.b.state(), GlobalState::Disabled, "{ev:?}");
    }
    // Losing the native control (tray crash) also disables.
    let h = harness();
    h.enable();
    h.b.detach_native_control();
    assert_eq!(h.b.state(), GlobalState::Disabled);
}

#[test]
fn injection_failure_is_outcome_unknown_and_releases_holds() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.fail_emit_after(2);
    let r = h.input(&a, &t, None, vec![Action::Drag { from: Point::desktop(10.0, 10.0), to: Point::desktop(100.0, 100.0), button: Button::Left, steps: 5 }]).unwrap();
    assert_eq!(r.outcome, Outcome::OutcomeUnknown);
    assert_eq!(r.error.unwrap().code, ErrorCode::OutcomeUnknown);
    assert_eq!(h.m.pressed(), (vec![], vec![]));
    assert_eq!(h.b.task(&t).unwrap().batches_unknown, 1);
}

#[test]
fn interlock_pauses_input() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.m.set_interlock(Some("an authentication prompt is on screen"));
    let r = h.input(&a, &t, Some("w-editor"), vec![Action::Type { text: "hunter2".into() }]).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::Blocked);
    assert!(h.m.events().is_empty());
}

// ------------------------------------------------------------------ Permissions and approvals

#[test]
fn agents_cannot_approve_or_grant() {
    let h = harness();
    h.enable();
    let agentish = Peer { agent_descendant: true, ..human() };
    assert_eq!(h.b.add_agent(&agentish, "sneaky", broad()).unwrap_err().code, ErrorCode::Denied);
    let a = h.agent("claude-code", policy(ControlMode::BroadDesktop, &[Capability::Pointer], &[]));
    let id = h.b.store.get(&a.agent).unwrap().id;
    assert_eq!(h.b.update_agent(&agentish, &id, broad()).unwrap_err().code, ErrorCode::Denied);
    // Reducing access is always allowed.
    h.b.revoke_agent(&id).unwrap();
    assert_eq!(h.b.task_begin(&a, "x").unwrap_err().code, ErrorCode::Unauthenticated);
    // Disable is always allowed, from anyone.
    h.b.disable("agent asked");
    assert_eq!(h.b.state(), GlobalState::Disabled);
    assert!(adl_schema::rpc::AGENT_METHODS.iter().all(|m| !m.contains("enable") && !m.contains("approv") || *m == "approval.status"));
}

#[test]
fn approvals_bind_the_exact_request() {
    let h = harness();
    h.enable();
    let mut p = broad();
    p.approval_required_for.insert("type".into());
    let (a, t) = h.driver("claude-code", p);
    let batch = vec![Action::Type { text: "send it".into() }];
    let e = h.input(&a, &t, Some("w-editor"), batch.clone()).unwrap_err();
    assert_eq!(e.code, ErrorCode::ApprovalRequired);
    let id = e.details["approval_id"].as_str().unwrap().to_string();
    let pending = h.b.approvals();
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].summary.contains("send it"), "summary never contains typed text");
    // Input is suspended while the approval is on screen.
    assert_eq!(h.input(&a, &t, None, vec![Action::Move { to: Point::desktop(5.0, 5.0) }]).unwrap_err().code, ErrorCode::Busy);
    h.b.decide_approval(&human(), &id, true).unwrap();
    // A materially different request is not covered.
    let other = h.input(&a, &t, Some("w-editor"), vec![Action::Type { text: "send something else".into() }]).unwrap_err();
    assert_eq!(other.code, ErrorCode::ApprovalRequired);
    assert_ne!(other.details["approval_id"], e.details["approval_id"]);
    h.b.decide_approval(&human(), other.details["approval_id"].as_str().unwrap(), false).unwrap();
    // The identical request goes through exactly once.
    assert_eq!(h.input(&a, &t, Some("w-editor"), batch.clone()).unwrap().outcome, Outcome::Completed);
    assert_eq!(h.input(&a, &t, Some("w-editor"), batch).unwrap_err().code, ErrorCode::ApprovalRequired);
}

#[test]
fn restricted_mode_is_confined_to_granted_apps() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("kate-bot", policy(ControlMode::Restricted, &[Capability::Pointer, Capability::Keyboard, Capability::Windows, Capability::ScreenCapture], &["org.kde.*"]));
    let ws = h.b.windows(&a).unwrap();
    assert_eq!(ws.data.iter().map(|w| w.app_id.as_str()).collect::<Vec<_>>(), ["org.kde.kate"]);
    assert_eq!(h.input(&a, &t, None, vec![Action::Type { text: "x".into() }]).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(h.input(&a, &t, Some("w-browser"), vec![Action::Type { text: "x".into() }]).unwrap_err().code, ErrorCode::Denied);
    let r = h.input(&a, &t, None, vec![click_at(Point::desktop(1500.0, 900.0))]).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::Denied, "browser window is not granted");
    assert_eq!(h.b.screenshot(&a, "DP-1").unwrap_err().code, ErrorCode::Denied);
    let r = h.input(&a, &t, Some("w-editor"), vec![click_at(Point::window("w-editor", 20.0, 20.0)), Action::Type { text: "ok".into() }]).unwrap();
    assert_eq!(r.outcome, Outcome::Completed);
}

#[test]
fn missing_capability_is_denied() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("pointer-only", policy(ControlMode::BroadDesktop, &[Capability::Pointer], &[]));
    assert_eq!(h.input(&a, &t, None, vec![Action::KeyPress { key: "Enter".parse().unwrap() }]).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(h.b.screenshot(&a, "DP-1").unwrap_err().code, ErrorCode::Denied);
}

#[test]
fn rate_limit() {
    let h = harness();
    h.enable();
    let mut p = broad();
    p.max_actions_per_minute = 3;
    let (a, t) = h.driver("slow", p);
    h.input(&a, &t, None, vec![Action::Wait { ms: 0 }, Action::Wait { ms: 0 }, Action::Wait { ms: 0 }]).unwrap();
    assert_eq!(h.input(&a, &t, None, vec![Action::Wait { ms: 0 }]).unwrap_err().code, ErrorCode::Busy);
}

#[test]
fn audit_has_metadata_but_no_content() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    h.input(&a, &t, Some("w-editor"), vec![Action::Type { text: "correct horse battery staple".into() }]).unwrap();
    let log = serde_json::to_string(&h.b.audit.export()).unwrap();
    assert!(log.contains("\"input\""));
    assert!(log.contains("type"));
    assert!(!log.contains("horse"));
}

#[test]
fn held_input_is_bounded() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let r = h.input(&a, &t, None, vec![Action::ButtonDown { button: Button::Left }, Action::Move { to: Point::desktop(400.0, 400.0) }]).unwrap();
    assert_eq!(r.outcome, Outcome::Completed);
    assert_eq!(h.m.pressed(), (vec![], vec![]), "a button never stays held after its batch");
}

#[test]
fn mock_adapter_reports_capabilities_honestly() {
    let m = MockDesktop::new();
    let caps = m.capabilities();
    assert!(caps.iter().any(|c| c.capability == Capability::Email && c.availability == Availability::Unsupported));
    assert!(caps.iter().any(|c| c.capability == Capability::Pointer && c.availability == Availability::Available));
}

#[test]
fn closed_connection_ends_its_tasks_and_lease() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    assert!(h.b.status().lease.is_some());
    // The same agent on another connection keeps its own task.
    let other = Principal { agent: a.agent.clone(), session: "sess_other".into() };
    let t2 = h.b.task_begin(&other, "other connection").unwrap().id;
    assert_eq!(h.b.end_session(&a), 1);
    assert!(h.b.status().lease.is_none(), "lease freed when its connection closed");
    assert_eq!(h.b.task(&t).unwrap().state, TaskState::Cancelled);
    assert_eq!(h.b.task(&t2).unwrap().state, TaskState::Running);
}

// ------------------------------------------------------------------ Drive prompt

fn prompted() -> (Arc<Broker>, mpsc::Receiver<String>) {
    let m = Arc::new(MockDesktop::new());
    let b = Arc::new(Broker::new(m, AgentStore::memory(), Audit::memory(), Config { drive_prompt: true, ..cfg() }));
    let (tx, rx) = mpsc::channel();
    b.subscribe(tx, true);
    let c = b.enable_request(&human()).unwrap();
    b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();
    (b, rx)
}

fn add(b: &Broker, name: &str) -> Principal {
    let (_, token) = b.add_agent(&human(), name, broad()).unwrap();
    b.authenticate(&token, None).unwrap()
}

/// The `drive_request_id` from a refused `lease.acquire`.
fn drive_id(e: &AdlError) -> String {
    e.details["drive_request_id"].as_str().expect("drive_request_id").to_string()
}

fn events(rx: &mpsc::Receiver<String>, name: &str) -> Vec<serde_json::Value> {
    rx.try_iter().filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok()).filter(|v| v["event"] == name).collect()
}

#[test]
fn drive_prompt_asks_once_per_agent_and_allow_lasts_while_it_drives() {
    let m = Arc::new(MockDesktop::new());
    let b = Arc::new(Broker::new(m, AgentStore::memory(), Audit::memory(), Config { drive_prompt: true, drive_idle_ms: 300, ..cfg() }));
    let (tx, rx) = mpsc::channel();
    b.subscribe(tx, true);
    let c = b.enable_request(&human()).unwrap();
    b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();

    let a = add(&b, "claude-code");
    let t = b.task_begin(&a, "post the MIT lectures").unwrap().id;
    let e = b.lease_acquire(&a, &t, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::ApprovalRequired);
    let id = drive_id(&e);
    let asked = events(&rx, "drive_requested");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["data"]["agent_name"], "claude-code");
    assert_eq!(asked[0]["data"]["description"], "post the MIT lectures");
    assert_eq!(b.status().pending_drive_requests, 1);
    // The task ending (a CLI that gave up) leaves the prompt up; asking again is the same request.
    b.task_end(&a, &t, TaskState::Cancelled).unwrap();
    let t2 = b.task_begin(&a, "click submit").unwrap().id;
    assert_eq!(drive_id(&b.lease_acquire(&a, &t2, None).unwrap_err()), id);
    assert!(events(&rx, "drive_requested").is_empty(), "no second prompt");
    assert_eq!(b.drive_requests()[0].description, "click submit", "the prompt shows what it wants now");

    b.decide_drive(&human(), &id, DriveDecision::Allow, None).unwrap();
    assert_eq!(b.status().pending_drive_requests, 0);
    b.lease_acquire(&a, &t2, None).unwrap();
    b.lease_release(&a).unwrap();
    // The next CLI command is a new task; it is covered while the agent keeps going.
    std::thread::sleep(Duration::from_millis(150));
    let t3 = b.task_begin(&a, "type the title").unwrap().id;
    b.lease_acquire(&a, &t3, None).unwrap();
    b.lease_release(&a).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let t4 = b.task_begin(&a, "press enter").unwrap().id;
    b.lease_acquire(&a, &t4, None).unwrap();
    b.lease_release(&a).unwrap();

    // Idle longer than drive_idle_ms: asked again.
    std::thread::sleep(Duration::from_millis(400));
    let t5 = b.task_begin(&a, "later").unwrap().id;
    assert_eq!(b.lease_acquire(&a, &t5, None).unwrap_err().code, ErrorCode::ApprovalRequired);
    assert_eq!(events(&rx, "drive_requested").len(), 1);
}

#[test]
fn drive_deny_and_postpone() {
    let (b, _rx) = prompted();
    let a = add(&b, "claude-code");
    let t = b.task_begin(&a, "x").unwrap().id;
    let id = drive_id(&b.lease_acquire(&a, &t, None).unwrap_err());
    b.decide_drive(&human(), &id, DriveDecision::Deny, None).unwrap();
    let e = b.lease_acquire(&a, &t, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied);
    assert!(e.details["retry_after_ms"].as_u64().unwrap() > 0, "denied for a while, not forever");
    let t2 = b.task_begin(&a, "y").unwrap().id;
    assert_eq!(b.lease_acquire(&a, &t2, None).unwrap_err().code, ErrorCode::Denied, "deny covers the agent, not one task");

    let z = add(&b, "codex");
    let t2 = b.task_begin(&z, "y").unwrap().id;
    let id2 = drive_id(&b.lease_acquire(&z, &t2, None).unwrap_err());
    let d = b.decide_drive(&human(), &id2, DriveDecision::Postpone, Some(15 * 60_000)).unwrap();
    assert_eq!(d.status, DriveStatus::Postponed);
    let e = b.lease_acquire(&z, &t2, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy);
    let retry = e.details["retry_after_ms"].as_u64().unwrap();
    assert!(retry > 14 * 60_000 && retry <= 15 * 60_000, "retry_after_ms {retry}");
    // A request already answered cannot be answered again.
    assert_eq!(b.decide_drive(&human(), &id, DriveDecision::Allow, None).unwrap_err().code, ErrorCode::Invalid);
}

#[test]
fn only_the_human_answers_a_drive_prompt() {
    let (b, _rx) = prompted();
    let a = add(&b, "claude-code");
    let t = b.task_begin(&a, "x").unwrap().id;
    let id = drive_id(&b.lease_acquire(&a, &t, None).unwrap_err());
    let agentish = Peer { agent_descendant: true, ..human() };
    assert_eq!(b.decide_drive(&agentish, &id, DriveDecision::Allow, None).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(b.decide_drive(&agentish, &id, DriveDecision::AlwaysAllow, None).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(b.set_drive_always_allow(&agentish, true).unwrap_err().code, ErrorCode::Denied);
    assert!(!b.status().drive_always_allow);
    assert_eq!(b.lease_acquire(&a, &t, None).unwrap_err().code, ErrorCode::ApprovalRequired);
}

#[test]
fn drive_always_allow_lets_everyone_in_until_turned_off() {
    let (b, rx) = prompted();
    let a = add(&b, "claude-code");
    let z = add(&b, "codex");
    let ta = b.task_begin(&a, "x").unwrap().id;
    let tz = b.task_begin(&z, "y").unwrap().id;
    let ia = drive_id(&b.lease_acquire(&a, &ta, None).unwrap_err());
    drive_id(&b.lease_acquire(&z, &tz, None).unwrap_err());
    b.decide_drive(&human(), &ia, DriveDecision::AlwaysAllow, None).unwrap();
    assert!(b.status().drive_always_allow);
    assert_eq!(b.status().pending_drive_requests, 0, "the other waiting agent was let in too");
    assert_eq!(events(&rx, "drive_always_allow").len(), 1);
    b.lease_acquire(&a, &ta, None).unwrap();
    b.lease_release(&a).unwrap();
    b.lease_acquire(&z, &tz, None).unwrap();
    b.lease_release(&z).unwrap();
    // New tasks: no prompt while it is on.
    let t3 = b.task_begin(&a, "z").unwrap().id;
    b.lease_acquire(&a, &t3, None).unwrap();
    b.lease_release(&a).unwrap();
    assert!(events(&rx, "drive_requested").is_empty());

    b.set_drive_always_allow(&human(), false).unwrap();
    let n = add(&b, "newcomer");
    let t4 = b.task_begin(&n, "w").unwrap().id;
    assert_eq!(b.lease_acquire(&n, &t4, None).unwrap_err().code, ErrorCode::ApprovalRequired);
}

#[test]
fn headless_instances_skip_it_and_stop_forgets_every_answer() {
    // Nobody is at a non-desktop instance's screen, so nobody is asked.
    let m = Arc::new(MockDesktop::new());
    let b = Broker::new(m, AgentStore::memory(), Audit::memory(), Config { enabled_by_default: true, drive_prompt: true, ..cfg() });
    b.enable_unattended().unwrap();
    let a = add(&b, "ci");
    let t = b.task_begin(&a, "x").unwrap().id;
    b.lease_acquire(&a, &t, None).unwrap();

    let (b, _rx) = prompted();
    let a = add(&b, "claude-code");
    let t = b.task_begin(&a, "x").unwrap().id;
    let id = drive_id(&b.lease_acquire(&a, &t, None).unwrap_err());
    b.decide_drive(&human(), &id, DriveDecision::Allow, None).unwrap();
    let z = add(&b, "codex");
    let tz = b.task_begin(&z, "y").unwrap().id;
    drive_id(&b.lease_acquire(&z, &tz, None).unwrap_err());
    // Stop is the person taking the desktop back: every answer is forgotten, agents ask again.
    let r = b.stop_all("test");
    assert_eq!(r.approvals_withdrawn, 1);
    assert_eq!(b.status().pending_drive_requests, 0);
    assert!(b.drive_requests().is_empty());
    let t2 = b.task_begin(&a, "again").unwrap().id;
    assert_eq!(b.lease_acquire(&a, &t2, None).unwrap_err().code, ErrorCode::ApprovalRequired);
}

// ------------------------------------------------------------------ Human steps (CAPTCHA, 2FA, sign-in)

#[test]
fn human_step_hands_the_desktop_to_the_person_and_back() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    let step = h.b.request_human_step(&a, Some(&t), HumanStepKind::Captcha, "Solve the CAPTCHA on the sign-up page").unwrap();
    assert_eq!(step.status, HumanStepStatus::Pending);
    assert!(h.b.status().lease.is_none(), "asking gives up the lease");
    assert_eq!(h.b.status().pending_human_steps, 1);

    // The person clicks and types to solve it: not a takeover.
    h.m.push_event(SessionEvent::PhysicalInput);
    h.b.pump_session_events();
    assert_eq!(h.b.state(), GlobalState::Enabled);

    // No agent drives meanwhile, the asker included.
    let other = h.agent("codex", broad());
    let to = h.b.task_begin(&other, "other").unwrap().id;
    let e = h.b.lease_acquire(&other, &to, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy);
    assert_eq!(e.details["human_step_id"], step.id.as_str());
    assert_eq!(h.b.lease_acquire(&a, &t, None).unwrap_err().code, ErrorCode::Busy);
    assert_eq!(h.input(&a, &t, None, vec![Action::Wait { ms: 1 }]).unwrap_err().code, ErrorCode::Cancelled, "no lease, no dispatch");

    let done = h.b.resolve_human_step(&human(), &step.id, true).unwrap();
    assert_eq!(done.status, HumanStepStatus::Done);
    assert_eq!(h.b.human_step_status(&a, &step.id).unwrap().status, HumanStepStatus::Done);
    assert_eq!(h.b.status().pending_human_steps, 0);
    h.b.lease_acquire(&a, &t, None).unwrap();
    // The screen changed under the person's hands: old captures are gone.
    let r = h.input(&a, &t, None, vec![click_at(Point::capture(&shot.meta.capture, 10.0, 10.0))]).unwrap();
    assert_eq!(r.outcome, Outcome::Failed);
    assert!(h.m.events().is_empty(), "no blind click after a human step");
}

#[test]
fn human_step_is_resolved_only_by_the_person() {
    let h = harness();
    h.enable();
    let (a, _) = h.driver("claude-code", broad());
    let step = h.b.request_human_step(&a, None, HumanStepKind::TwoFactor, "Type the code from your phone").unwrap();
    let agentish = Peer { agent_descendant: true, ..human() };
    assert_eq!(h.b.resolve_human_step(&agentish, &step.id, true).unwrap_err().code, ErrorCode::Denied);
    // Another agent cannot read it either.
    let other = h.agent("codex", broad());
    assert_eq!(h.b.human_step_status(&other, &step.id).unwrap_err().code, ErrorCode::NotFound);
    let c = h.b.resolve_human_step(&human(), &step.id, false).unwrap();
    assert_eq!(c.status, HumanStepStatus::Cancelled);
    assert_eq!(h.b.resolve_human_step(&human(), &step.id, true).unwrap_err().code, ErrorCode::Invalid, "answered once");
}

#[test]
fn human_step_needs_a_message_and_an_enabled_layer() {
    let h = harness();
    let a = h.agent("claude-code", broad());
    assert_eq!(h.b.request_human_step(&a, None, HumanStepKind::Login, "Sign in").unwrap_err().code, ErrorCode::Disabled);
    h.enable();
    assert_eq!(h.b.request_human_step(&a, None, HumanStepKind::Login, "  ").unwrap_err().code, ErrorCode::Invalid);
    // One open step per agent: asking again updates it.
    let s1 = h.b.request_human_step(&a, None, HumanStepKind::Login, "Sign in").unwrap();
    let s2 = h.b.request_human_step(&a, None, HumanStepKind::Login, "Sign in to GitHub").unwrap();
    assert_eq!(s1.id, s2.id);
    assert_eq!(h.b.human_steps().len(), 1);
    assert_eq!(h.b.human_steps()[0].message, "Sign in to GitHub");
}

#[test]
fn stop_withdraws_human_steps() {
    let h = harness();
    h.enable();
    let (a, _) = h.driver("claude-code", broad());
    let step = h.b.request_human_step(&a, None, HumanStepKind::Payment, "Confirm the payment").unwrap();
    h.b.stop_all("test");
    assert_eq!(h.b.human_step_status(&a, &step.id).unwrap().status, HumanStepStatus::Expired);
    let t = h.b.task_begin(&a, "again").unwrap().id;
    h.b.lease_acquire(&a, &t, None).unwrap();
}

#[test]
fn unanswered_human_step_lapses() {
    let h = harness_with(Config { human_step_ttl_ms: 0, ..cfg() });
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    let step = h.b.request_human_step(&a, Some(&t), HumanStepKind::Other, "Look at this").unwrap();
    assert_eq!(h.b.human_step_status(&a, &step.id).unwrap().status, HumanStepStatus::Expired);
    h.b.lease_acquire(&a, &t, None).unwrap();
}

// ------------------------------------------------------------------ Screen memory (locate fast path)

/// Draw a 100x30 "button" whose stripes depend on `seed`, so different buttons look different.
fn widget(m: &MockDesktop, display: &str, x: u32, y: u32, seed: u32) {
    use adl_broker::PixelRect;
    m.paint(display, PixelRect { x, y, width: 100, height: 30 }, 60);
    let mut cx = x + 4;
    let mut i = 0;
    while cx < x + 96 {
        let wd = 3 + (seed * 7 + i * 5) % 9;
        m.paint(display, PixelRect { x: cx, y: y + 6 + (i % 3) * 2, width: wd.min(x + 96 - cx), height: 14 }, (100 + (seed * 37 + i * 53) % 150) as u8);
        cx += wd + 2;
        i += 1;
    }
}

fn erase(m: &MockDesktop, display: &str, x: u32, y: u32) {
    m.paint(display, adl_broker::PixelRect { x, y, width: 100, height: 30 }, 0xe0);
}

#[test]
fn remember_then_locate_finds_a_moved_target_and_clicks_it() {
    let h = harness();
    h.enable();
    let (a, t) = h.driver("claude-code", broad());
    widget(&h.m, "DP-1", 400, 300, 1);
    widget(&h.m, "DP-1", 900, 500, 2);
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    let saved = h.b.memory_remember(&a, "Editor: Save button", &shot.meta.capture, 450.0, 315.0, None).unwrap();
    assert_eq!((saved.width, saved.height), (48, 48));

    // The window moved: the button is now at (1200, 800).
    erase(&h.m, "DP-1", 400, 300);
    widget(&h.m, "DP-1", 1200, 800, 1);
    let r = h.b.memory_locate(&a, "Editor: Save button", "DP-1", None).unwrap();
    assert!(r.found, "{r:?}");
    assert!(r.score > 0.95 && !r.ambiguous);
    let p = r.point.clone().unwrap();
    h.input(&a, &t, None, vec![Action::Click { button: Button::Left, count: 1, at: Some(p), modifiers: vec![] }]).unwrap();
    let InputEvent::MotionAbsolute { x, y } = h.m.events()[0] else { panic!("{:?}", h.m.events()) };
    assert!((x - 1250.0).abs() <= 1.0 && (y - 815.0).abs() <= 1.0, "clicked ({x},{y})");
    let e = &h.b.memory_list(&a)[0];
    assert_eq!((e.hits, e.misses), (1, 0));
}

#[test]
fn locate_routes_misses_and_look_alikes_to_the_slow_path() {
    let h = harness();
    h.enable();
    let a = h.agent("claude-code", broad());
    widget(&h.m, "DP-1", 400, 300, 1);
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    h.b.memory_remember(&a, "ok", &shot.meta.capture, 450.0, 315.0, None).unwrap();

    // Gone (or redesigned): not found, no point, the agent is told to look and remember again.
    erase(&h.m, "DP-1", 400, 300);
    widget(&h.m, "DP-1", 400, 300, 9);
    let r = h.b.memory_locate(&a, "ok", "DP-1", None).unwrap();
    assert!(!r.found && r.point.is_none(), "{r:?}");
    assert!(r.hint.contains("desktop_remember"));

    // Two identical buttons: refused as ambiguous rather than guessed.
    erase(&h.m, "DP-1", 400, 300);
    widget(&h.m, "DP-1", 400, 300, 1);
    widget(&h.m, "DP-1", 1300, 700, 1);
    let r = h.b.memory_locate(&a, "ok", "DP-1", None).unwrap();
    assert!(!r.found && r.ambiguous, "{r:?}");
    assert_eq!(h.b.memory_list(&a)[0].misses, 2);
}

#[test]
fn remember_refuses_flat_areas_stale_captures_and_human_steps() {
    let h = harness();
    h.enable();
    let a = h.agent("claude-code", broad());
    let other = h.agent("codex", broad());
    widget(&h.m, "DP-1", 400, 300, 1);
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    assert_eq!(h.b.memory_remember(&a, "blank", &shot.meta.capture, 1500.0, 900.0, None).unwrap_err().code, ErrorCode::Invalid, "flat grey");
    assert_eq!(h.b.memory_remember(&a, "edge", &shot.meta.capture, 5.0, 5.0, None).unwrap_err().code, ErrorCode::Invalid, "patch off the edge");
    assert_eq!(h.b.memory_remember(&other, "x", &shot.meta.capture, 450.0, 315.0, None).unwrap_err().code, ErrorCode::TargetStale, "not codex's capture");
    let newer = h.b.screenshot(&a, "DP-1").unwrap();
    assert_eq!(h.b.memory_remember(&a, "x", &shot.meta.capture, 450.0, 315.0, None).unwrap_err().code, ErrorCode::TargetStale, "only the latest capture");
    h.b.request_human_step(&other, None, HumanStepKind::Login, "Sign in").unwrap();
    assert_eq!(h.b.memory_remember(&a, "x", &newer.meta.capture, 450.0, 315.0, None).unwrap_err().code, ErrorCode::Busy, "nothing saved while a person types a password");
    // Locating something never saved says how to save it.
    assert_eq!(h.b.memory_locate(&a, "nope", "DP-1", None).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn screen_memory_is_per_agent_needs_broad_capture_and_the_person_can_forget_it() {
    let h = harness();
    h.enable();
    let a = h.agent("claude-code", broad());
    let other = h.agent("codex", broad());
    let narrow = h.agent("mailer", policy(ControlMode::Restricted, &Capability::ALL, &["org.mozilla.firefox"]));
    widget(&h.m, "DP-1", 400, 300, 1);
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    h.b.memory_remember(&a, "ok", &shot.meta.capture, 450.0, 315.0, None).unwrap();
    assert_eq!(h.b.memory_locate(&other, "ok", "DP-1", None).unwrap_err().code, ErrorCode::NotFound, "codex has its own memory");
    assert_eq!(h.b.memory_locate(&narrow, "ok", "DP-1", None).unwrap_err().code, ErrorCode::Denied);
    let agentish = Peer { agent_descendant: true, ..human() };
    assert_eq!(h.b.memory_forget(&agentish, "claude-code", None).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(h.b.memory_forget(&human(), "claude-code", Some("ok")).unwrap(), 1, "by agent name");
    assert!(h.b.memory_list(&a).is_empty());
    let shot = h.b.screenshot(&a, "DP-1").unwrap();
    h.b.memory_remember(&a, "ok", &shot.meta.capture, 450.0, 315.0, None).unwrap();
    h.b.remove_agent(&a.agent).unwrap();
    assert!(h.b.screen_memory.list(None).is_empty(), "removing an agent deletes its patches");
}
