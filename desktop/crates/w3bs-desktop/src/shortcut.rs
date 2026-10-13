//! Emergency-stop shortcut through `org.freedesktop.portal.GlobalShortcuts` (KDE Plasma 6, GNOME
//! 48+). The desktop owns the binding and may ask the human to confirm or change it; we only
//! listen for `Activated`. Detecting the shortcut needs no keyboard monitoring, so this is not a
//! keylogger (PRD section 6).

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
pub const SHORTCUT_ID: &str = "emergency-stop";
const PREFERRED: &str = "SUPER+SHIFT+Escape";

type Results = HashMap<String, OwnedValue>;

fn wait_response(conn: &Connection, token: &str, call: impl FnOnce() -> zbus::Result<OwnedObjectPath>) -> Result<Results, String> {
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).ok_or("no bus name")?;
    let req = Proxy::new(conn, DEST, format!("{PATH}/request/{sender}/{token}"), "org.freedesktop.portal.Request").map_err(|e| e.to_string())?;
    let it = req.receive_signal("Response").map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(m) = it.into_iter().next() {
            let _ = tx.send(m.body().deserialize::<(u32, Results)>().map_err(|e| e.to_string()));
        }
    });
    call().map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok((0, r))) => Ok(r),
        Ok(Ok((code, _))) => Err(format!("refused (response {code})")),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("no answer from the shortcut dialog".into()),
    }
}

/// Bind the shortcut and forward activations as `Quit` (disable and exit) to the tray.
pub fn register<A>(tx: mpsc::Sender<A>) -> Result<String, String>
where
    A: From<Emergency> + Send + 'static,
{
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let gs = Proxy::new(&conn, DEST, PATH, IFACE).map_err(|e| e.to_string())?;
    let created = wait_response(&conn, "adlgs1", || {
        let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from("adlgs1")), ("session_handle_token", Value::from("adlgss"))]);
        gs.call("CreateSession", &(opts,))
    })?;
    let session: String = created.get("session_handle").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).ok_or("no session handle")?;
    let session = OwnedObjectPath::try_from(session).map_err(|e| e.to_string())?;
    let bound = wait_response(&conn, "adlgs2", || {
        let props: HashMap<&str, Value> = HashMap::from([("description", Value::from("Stop all agent activity and disable agent access")), ("preferred_trigger", Value::from(PREFERRED))]);
        let shortcuts = vec![(SHORTCUT_ID, props)];
        let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from("adlgs2"))]);
        gs.call("BindShortcuts", &(&session, shortcuts, "", opts))
    })?;
    let trigger = describe(&bound).unwrap_or_else(|| PREFERRED.replace('+', " + "));
    let it = gs.receive_signal("Activated").map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _keep = conn;
        for m in it {
            if let Ok((_, id, _, _)) = m.body().deserialize::<(OwnedObjectPath, String, u64, Results)>()
                && id == SHORTCUT_ID
                && tx.send(A::from(Emergency)).is_err()
            {
                break;
            }
        }
    });
    Ok(trigger)
}

/// The trigger the desktop actually assigned, from BindShortcuts' `shortcuts` result.
fn describe(r: &Results) -> Option<String> {
    let v = Value::from(r.get("shortcuts")?.try_clone().ok()?);
    let Value::Array(arr) = v else { return None };
    for s in arr.iter() {
        let Value::Structure(st) = s else { continue };
        if let [Value::Str(id), Value::Dict(d)] = st.fields()
            && id.as_str() == SHORTCUT_ID
        {
            let d: HashMap<String, OwnedValue> = d.try_clone().ok()?.try_into().ok()?;
            return String::try_from(d.get("trigger_description")?.try_clone().ok()?).ok();
        }
    }
    None
}

/// Marker sent when the emergency shortcut fires.
pub struct Emergency;
