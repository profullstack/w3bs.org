//! The broker over the KWin observer, fed canned script reports: focus guards keyboard input,
//! stacking guards clicks, the app grant uses KWin's app ids, and the polkit prompt pauses input.

use adl_broker::audit::Audit;
use adl_broker::mock::MockDesktop;
use adl_broker::store::AgentStore;
use adl_broker::{Broker, Config, Peer, Principal};
use adl_linux::kwin::{KWinObserver, KWinSnapshot, KWinSource, WithKWin};
use adl_schema::*;
use std::sync::{Arc, Mutex};

/// T1 (foot) below T2 (foot, focused), a kwrite window off to the right.
const REPORT: &str = r#"{
  "source": "stackingOrder", "active": "2222",
  "windows": [
    {"id": "1111", "resourceClass": "foot", "desktopFileName": "foot", "caption": "T1", "x": 100, "y": 100, "width": 600, "height": 400, "normal": true},
    {"id": "3333", "resourceClass": "kwrite", "desktopFileName": "org.kde.kwrite", "caption": "notes", "x": 1200, "y": 100, "width": 400, "height": 400, "normal": true},
    {"id": "2222", "resourceClass": "foot", "desktopFileName": "foot", "caption": "T2", "x": 400, "y": 300, "width": 600, "height": 400, "normal": true}
  ]
}"#;

struct Canned(Arc<Mutex<String>>);
impl KWinSource for Canned {
    fn snapshot(&self) -> Result<KWinSnapshot> {
        KWinSnapshot::parse(&self.0.lock().unwrap())
    }
    fn activate(&self, _: &str) -> Result<bool> {
        Ok(true)
    }
}

fn human() -> Peer {
    Peer { pid: Some(1), agent_descendant: false, interactive: true, native_control: true }
}

type Events = std::sync::mpsc::Receiver<String>;

fn setup() -> (Broker, Arc<Mutex<String>>, Events) {
    let report = Arc::new(Mutex::new(REPORT.to_string()));
    let a = WithKWin::new(MockDesktop::new(), KWinObserver::new(Box::new(Canned(report.clone()))));
    let b = Broker::new(Arc::new(a), AgentStore::memory(), Audit::memory(), Config { pace_ms: 0, ..Config::default() });
    // A native control must be listening before a human can enable.
    let (tx, rx) = std::sync::mpsc::channel();
    b.subscribe(tx, true);
    let c = b.enable_request(&human()).unwrap();
    b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();
    (b, report, rx)
}

fn agent(b: &Broker, name: &str, mode: ControlMode, apps: &[&str]) -> Principal {
    let pol = Policy { capabilities: Capability::ALL.into_iter().collect(), mode, apps: apps.iter().map(|s| s.to_string()).collect(), ..Default::default() };
    let (_, token) = b.add_agent(&human(), name, pol).unwrap();
    b.authenticate(&token, None).unwrap()
}

/// Run one batch; the error code it ended with, if any.
fn run(b: &Broker, p: &Principal, window: Option<&str>, action: Action) -> Option<(ErrorCode, String)> {
    let t = b.task_begin(p, "t").unwrap().id;
    b.lease_acquire(p, &t, None).unwrap();
    let r = b.input(p, &InputBatch { task: t.clone(), target: Target { window: window.map(str::to_string), layout_generation: None }, actions: vec![action] });
    let _ = b.lease_release(p);
    match r {
        Ok(r) => r.error.map(|e| (e.code, e.message)),
        Err(e) => Some((e.code, e.message)),
    }
}

fn type_(text: &str) -> Action {
    Action::Type { text: text.into() }
}

fn click(at: Point) -> Action {
    Action::Click { button: Button::Left, count: 1, at: Some(at), modifiers: vec![] }
}

#[test]
fn focus_and_stacking_guard_targeted_input() {
    let (b, _, _events) = setup();
    let a = agent(&b, "a", ControlMode::BroadDesktop, &[]);
    let ws = b.windows(&a).unwrap().data;
    assert_eq!(ws.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), ["T2", "notes", "T1"]);
    assert_eq!(ws.iter().filter(|w| w.focused).count(), 1);

    assert_eq!(run(&b, &a, Some("2222"), type_("ok")), None, "focused target");
    let (code, msg) = run(&b, &a, Some("1111"), type_("no")).unwrap();
    assert_eq!(code, ErrorCode::TargetStale, "{msg}");
    assert!(msg.contains("focus moved away") && msg.contains("T2"), "{msg}");

    assert_eq!(run(&b, &a, Some("1111"), click(Point::window("1111", 15.0, 200.0))), None, "visible strip of T1");
    // (500, 400) in desktop terms lies in the overlap, where T2 is on top.
    let (code, msg) = run(&b, &a, Some("1111"), click(Point::window("1111", 400.0, 300.0))).unwrap();
    assert_eq!(code, ErrorCode::TargetStale, "{msg}");
    assert!(msg.contains("covered by T2 (foot)"), "{msg}");
}

#[test]
fn restricted_grant_uses_kwin_app_ids() {
    let (b, _, _events) = setup();
    let r = agent(&b, "r", ControlMode::Restricted, &["foot"]);
    assert_eq!(b.windows(&r).unwrap().data.len(), 2, "kwrite is filtered out");
    assert_eq!(run(&b, &r, Some("2222"), type_("ok")), None);
    let (code, msg) = run(&b, &r, Some("3333"), type_("no")).unwrap();
    assert_eq!(code, ErrorCode::Denied, "{msg}");
    let (code, msg) = run(&b, &r, None, click(Point::desktop(1300.0, 200.0))).unwrap();
    assert_eq!(code, ErrorCode::Denied, "{msg}");
    assert!(msg.contains("org.kde.kwrite"), "{msg}");
}

#[test]
fn polkit_prompt_pauses_input() {
    let (b, report, _events) = setup();
    let a = agent(&b, "a", ControlMode::BroadDesktop, &[]);
    *report.lock().unwrap() = REPORT.replace(r#""resourceClass": "kwrite""#, r#""resourceClass": "polkit-kde-authentication-agent-1""#);
    let (code, msg) = run(&b, &a, None, click(Point::desktop(50.0, 50.0))).unwrap();
    assert_eq!(code, ErrorCode::Blocked, "{msg}");
    assert!(msg.contains("authentication prompt"), "{msg}");
}
