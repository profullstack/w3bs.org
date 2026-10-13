//! Emergency-stop shortcut through `org.freedesktop.portal.GlobalShortcuts` (KDE Plasma 6, GNOME
//! 48+). The desktop owns the binding and may ask the human to confirm or change it; we only
//! listen for `Activated`. Detecting the shortcut needs no keyboard monitoring, so this is not a
//! keylogger (PRD section 6).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
pub const SHORTCUT_ID: &str = "emergency-stop";
/// XDG shortcuts-spec syntax: the Super key is `LOGO` (KDE rejects "SUPER": "Unknown modifier").
/// Super+Alt+Escape is free on Plasma and GNOME; the obvious neighbours are taken: KWin owns
/// Super+Shift+Escape ("Disable Active Input Capture", which wins and swallows the key),
/// Super+Ctrl+Escape and Ctrl+Alt+Escape (Kill Window); Mutter owns Super+Escape.
const PREFERRED: &str = "LOGO+ALT+Escape";
/// The app id the portal (and the desktop's shortcut settings) list the binding under.
pub const APP_ID: &str = "org.w3bs.Desktop";

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

/// Bind the shortcut and forward each activation to the tray as `A::from(Emergency)`.
pub fn register<A>(tx: mpsc::Sender<A>) -> Result<String, String>
where
    A: From<Emergency> + Send + 'static,
{
    let conn = Connection::session().map_err(|e| e.to_string())?;
    // GlobalShortcuts refuses unsandboxed callers without an app id ("An app id is required",
    // xdg-desktop-portal 1.19+). A host app names itself with the host Registry, on this same
    // connection and before any other portal call (a raw call: a Proxy would prefetch properties
    // first). The portal only accepts an id with an installed desktop entry. Older portals lack
    // the Registry; then BindShortcuts decides.
    let wrote = ensure_desktop_entry();
    let register = || conn.call_method(Some(DEST), PATH, Some("org.freedesktop.host.portal.Registry"), "Register", &(APP_ID, HashMap::<&str, Value>::new()));
    let mut reg = register();
    if reg.is_err() && wrote {
        std::thread::sleep(Duration::from_secs(2)); // the portal notices new desktop files by file monitor
        reg = register();
    }
    if let Err(e) = reg {
        eprintln!("w3bs-desktop tray: portal Registry.Register({APP_ID}): {e}");
    }
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
    // An empty description means the binding exists with no key (KDE keeps "none" once a human
    // dismissed the dialog); say so rather than show a shortcut that does nothing.
    let trigger = match describe(&bound) {
        Some(d) if !d.trim().is_empty() => d,
        Some(_) => "no key assigned (set one in the desktop's shortcut settings)".into(),
        None => "Super+Alt+Escape".into(),
    };
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

/// Install a hidden `org.w3bs.Desktop.desktop` in the user's data dir unless one is already on the
/// XDG data path. Returns whether it wrote one.
fn ensure_desktop_entry() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let user = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/share"));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    let file = format!("applications/{APP_ID}.desktop");
    if std::iter::once(user.clone()).chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).any(|d| d.join(&file).exists()) {
        return false;
    }
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "w3bs-desktop".into());
    let body = format!("[Desktop Entry]\nType=Application\nName=Agentic Desktop Layer\nComment=Native control for agent access to this desktop\nExec={exe} tray\nIcon=changes-prevent\nNoDisplay=true\n");
    let path = user.join(&file);
    path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&path, body).is_ok()
}

/// Marker sent when the emergency shortcut fires.
pub struct Emergency;
