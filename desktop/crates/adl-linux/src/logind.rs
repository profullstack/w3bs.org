//! Session events from systemd-logind: lock, unlock, VT switch away, suspend and logout.
//!
//! Cross-desktop (KDE, GNOME, Vela, wlroots). Note: Vela does not maintain `LockedHint`, so the
//! Vela adapter also polls its own lock state; logind's `Lock` signal and `PrepareForSleep`
//! still work there.

use adl_broker::SessionEvent;
use std::sync::{Arc, Mutex};
use zbus::blocking::{Connection, Proxy};

const DEST: &str = "org.freedesktop.login1";

#[derive(Clone, Default)]
pub struct LogindWatcher {
    events: Arc<Mutex<Vec<SessionEvent>>>,
}

impl LogindWatcher {
    /// Start background watchers. Failure to reach logind is not fatal; it is reported once so
    /// `doctor` can show it, and lock/suspend then rely on the native control.
    pub fn start() -> Result<Self, String> {
        let w = Self::default();
        let conn = Connection::system().map_err(|e| format!("system bus: {e}"))?;
        let manager = Proxy::new(&conn, DEST, "/org/freedesktop/login1", "org.freedesktop.login1.Manager").map_err(|e| e.to_string())?;
        let session_path: zbus::zvariant::OwnedObjectPath = match std::env::var("XDG_SESSION_ID") {
            Ok(id) => manager.call("GetSession", &(id,)),
            Err(_) => manager.call("GetSessionByPID", &(std::process::id(),)),
        }
        .map_err(|e| format!("logind session: {e}"))?;

        {
            let (conn, ev) = (conn.clone(), w.events.clone());
            std::thread::spawn(move || {
                let Ok(m) = Proxy::new(&conn, DEST, "/org/freedesktop/login1", "org.freedesktop.login1.Manager") else { return };
                let Ok(it) = m.receive_signal("PrepareForSleep") else { return };
                for msg in it {
                    if let Ok(true) = msg.body().deserialize::<bool>() {
                        ev.lock().unwrap().push(SessionEvent::Suspending);
                    }
                }
            });
        }
        for (signal, event) in [("Lock", SessionEvent::Locked), ("Unlock", SessionEvent::Unlocked)] {
            let (conn, ev, path) = (conn.clone(), w.events.clone(), session_path.clone());
            std::thread::spawn(move || {
                let Ok(s) = Proxy::new(&conn, DEST, path, "org.freedesktop.login1.Session") else { return };
                let Ok(it) = s.receive_signal(signal) else { return };
                for _ in it {
                    ev.lock().unwrap().push(event.clone());
                }
            });
        }
        {
            // Active=false (switched to another VT or user) and LockedHint=true both revoke.
            let (conn, ev, path) = (conn.clone(), w.events.clone(), session_path.clone());
            std::thread::spawn(move || {
                let Ok(props) = Proxy::new(&conn, DEST, path, "org.freedesktop.DBus.Properties") else { return };
                let Ok(it) = props.receive_signal("PropertiesChanged") else { return };
                for msg in it {
                    type Changed = (String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<String>);
                    let Ok((_, changed, _)) = msg.body().deserialize::<Changed>() else { continue };
                    let flag = |k: &str| changed.get(k).and_then(|v| bool::try_from(v).ok());
                    if flag("Active") == Some(false) || flag("LockedHint") == Some(true) {
                        ev.lock().unwrap().push(SessionEvent::Locked);
                    }
                }
            });
        }
        Ok(w)
    }

    pub fn drain(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}
