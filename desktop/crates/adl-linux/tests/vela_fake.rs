//! The Vela adapter against a fake command socket that speaks the documented line protocol.

use adl_broker::audit::Audit;
use adl_broker::store::AgentStore;
use adl_broker::{Adapter, Broker, Config, Peer};
use adl_linux::vela::VelaAdapter;
use adl_schema::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Fake {
    state: Arc<Mutex<Value>>,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

fn base_state() -> Value {
    json!({
        "locked": false, "workspace": 0, "workspaces": 1, "focused": "win-b", "held": 0,
        "outputs": [
            {"name": "DP-1", "x": 0, "y": 0, "w": 2560, "h": 1440, "scale": 1.5, "enabled": true, "powered": true, "frames": 10, "missed": 0},
            {"name": "HDMI-A-1", "x": -1920, "y": 0, "w": 1920, "h": 1080, "scale": 1.0, "enabled": true, "powered": true, "frames": 10, "missed": 0}
        ],
        "windows": [
            {"id": "win-b", "app": "org.kde.kate", "title": "Kate", "x": 100, "y": 100, "w": 800, "h": 600, "output": "DP-1", "workspace": 0, "sticky": false, "maximized": false, "minimized": false, "fullscreen": false, "snap": null, "snapGroup": 0},
            {"id": "win-a", "app": "firefox", "title": "Firefox", "x": 0, "y": 0, "w": 1200, "h": 900, "output": "DP-1", "workspace": 0, "sticky": false, "maximized": false, "minimized": false, "fullscreen": false, "snap": null, "snapGroup": 0}
        ],
        "layers": [{"namespace": "vela-panel", "layer": 2, "keyboard": 2, "mapped": true, "output": "DP-1"}],
        "focusedLayer": null
    })
}

fn rects(state: &Value) -> Value {
    Value::Array(state["windows"].as_array().unwrap().iter().filter(|w| !w["minimized"].as_bool().unwrap()).map(|w| json!({"id": w["id"], "title": w["title"], "x": w["x"], "y": w["y"], "w": w["w"], "h": w["h"]})).collect())
}

fn fake() -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vela-wayland-0.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let state = Arc::new(Mutex::new(base_state()));
    let st = state.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let st = st.clone();
            std::thread::spawn(move || {
                let mut w = conn.try_clone().unwrap();
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    let s = st.lock().unwrap().clone();
                    let reply = match line.as_str() {
                        "state" => Some(s.to_string()),
                        "window-rects" => Some(rects(&s).to_string()),
                        l if l.starts_with("window ") => {
                            let id = l.split_whitespace().nth(1).unwrap().to_string();
                            st.lock().unwrap()["focused"] = json!(id);
                            None
                        }
                        _ => None, // unknown lines are ignored silently, like the real socket
                    };
                    if let Some(r) = reply {
                        writeln!(w, "{r}").unwrap();
                    }
                }
            });
        }
    });
    Fake { state, path, _dir: dir }
}

fn human() -> Peer {
    Peer { pid: Some(1), agent_descendant: false, interactive: true, native_control: true }
}

#[test]
fn observes_layout_windows_and_occlusion() {
    let f = fake();
    let v = VelaAdapter::new(f.path.clone());
    v.begin().unwrap();
    let l = v.layout().unwrap();
    assert_eq!(l.displays.len(), 2);
    assert_eq!(l.displays[1].logical, Rect { x: -1920.0, y: 0.0, width: 1920.0, height: 1080.0 });
    assert_eq!(l.displays[0].scale, 1.5);
    let ws = v.windows().unwrap();
    assert!(ws.iter().find(|w| w.id == "win-b").unwrap().focused);
    // win-b is topmost and overlaps win-a.
    assert_eq!(v.window_at(150.0, 150.0).unwrap().unwrap().id, "win-b");
    assert_eq!(v.window_at(1000.0, 800.0).unwrap().unwrap().id, "win-a");
    assert!(v.window_at(-100.0, 50.0).unwrap().is_none());
    v.activate("win-a").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(v.focused_window().unwrap().unwrap().id, "win-a");
    assert!(v.activate("win-a lock").is_err(), "no command injection through ids");
}

#[test]
fn interlocks() {
    let f = fake();
    let v = VelaAdapter::new(f.path.clone());
    assert_eq!(v.safety_interlock(), None);
    f.state.lock().unwrap()["focusedLayer"] = json!("vela-polkit");
    assert!(v.safety_interlock().unwrap().contains("authentication prompt"));
    f.state.lock().unwrap()["focusedLayer"] = Value::Null;
    f.state.lock().unwrap()["layers"] = json!([{"namespace": "some-modal", "layer": 3, "keyboard": 1, "mapped": true}]);
    assert!(v.safety_interlock().unwrap().contains("some-modal"));
    f.state.lock().unwrap()["layers"] = json!([]);
    f.state.lock().unwrap()["locked"] = json!(true);
    assert!(v.safety_interlock().unwrap().contains("locked"));
}

#[test]
fn broker_with_vela_disables_on_lock_and_compositor_loss() {
    let f = fake();
    let v = Arc::new(VelaAdapter::new(f.path.clone()));
    let b = Broker::new(v.clone(), AgentStore::memory(), Audit::memory(), Config { pace_ms: 0, ..Config::default() });
    let (tx, _rx) = std::sync::mpsc::channel();
    b.subscribe(tx, true);
    let c = b.enable_request(&human()).unwrap();
    b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();
    let (_, token) = b.add_agent(&human(), "a", Policy { capabilities: Capability::ALL.into_iter().collect(), mode: ControlMode::BroadDesktop, ..Default::default() }).unwrap();
    let a = b.authenticate(&token, None).unwrap();
    assert_eq!(b.windows(&a).unwrap().data.len(), 2);

    // Input is honestly unsupported on stock Vela.
    let t = b.task_begin(&a, "t").unwrap().id;
    b.lease_acquire(&a, &t, None).unwrap();
    let r = b.input(&a, &InputBatch { task: t, target: Target::default(), actions: vec![Action::Move { to: Point::desktop(10.0, 10.0) }] }).unwrap();
    assert_eq!(r.error.unwrap().code, ErrorCode::OutcomeUnknown, "emit is refused by the backend");
    assert!(v.capabilities().iter().any(|c| c.capability == Capability::Pointer && c.availability == Availability::Unsupported));

    // Layout change bumps the generation.
    let g1 = v.layout().unwrap().generation;
    f.state.lock().unwrap()["outputs"][1]["enabled"] = json!(false);
    std::thread::sleep(std::time::Duration::from_millis(120));
    assert_eq!(v.layout().unwrap().generation, g1 + 1);

    // Locking disables.
    f.state.lock().unwrap()["locked"] = json!(true);
    b.pump_session_events();
    assert_eq!(b.state(), GlobalState::Disabled);

    // Compositor loss while enabled disables too.
    f.state.lock().unwrap()["locked"] = json!(false);
    let c = b.enable_request(&human()).unwrap();
    b.enable_confirm(&human(), c.code.as_deref().unwrap()).unwrap();
    b.pump_session_events();
    assert_eq!(b.state(), GlobalState::Enabled);
    std::fs::remove_file(&f.path).unwrap();
    b.pump_session_events();
    assert_eq!(b.state(), GlobalState::Disabled);
}
