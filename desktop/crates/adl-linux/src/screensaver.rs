//! Lock events from the desktop's own screen saver service, on the session bus.
//!
//! GNOME Shell emits `org.gnome.ScreenSaver.ActiveChanged(true)` when its shield goes up (lock,
//! or the blank that precedes it), and KDE Plasma's screen locker emits
//! `org.freedesktop.ScreenSaver.ActiveChanged(true)` when it locks. These work where logind is
//! absent or does not maintain `LockedHint`, and they arrive even while the locked shell has
//! unloaded its extensions. Any session-bus peer can emit such a signal, but the only effect is
//! disabling agent access, which is always safe; nothing here can enable anything.

use adl_broker::SessionEvent;
use std::sync::{Arc, Mutex};
use zbus::blocking::{Connection, MessageIterator};
use zbus::MatchRule;

const INTERFACES: &[&str] = &["org.gnome.ScreenSaver", "org.freedesktop.ScreenSaver"];

#[derive(Clone, Default)]
pub struct ScreenSaverWatcher {
    events: Arc<Mutex<Vec<SessionEvent>>>,
}

impl ScreenSaverWatcher {
    pub fn start() -> Result<Self, String> {
        let w = Self::default();
        let conn = Connection::session().map_err(|e| format!("session bus: {e}"))?;
        for iface in INTERFACES {
            let rule = MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(*iface)
                .and_then(|b| b.member("ActiveChanged"))
                .map_err(|e| e.to_string())?
                .build();
            let it = MessageIterator::for_match_rule(rule, &conn, Some(16)).map_err(|e| format!("{iface}: {e}"))?;
            let ev = w.events.clone();
            std::thread::spawn(move || {
                for msg in it.flatten() {
                    match msg.body().deserialize::<bool>() {
                        Ok(true) => ev.lock().unwrap_or_else(|e| e.into_inner()).push(SessionEvent::Locked),
                        Ok(false) => ev.lock().unwrap_or_else(|e| e.into_inner()).push(SessionEvent::Unlocked),
                        Err(_) => {}
                    }
                }
            });
        }
        Ok(w)
    }

    pub fn drain(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
