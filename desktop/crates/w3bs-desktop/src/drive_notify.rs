//! The drive prompt as a desktop notification (`org.freedesktop.Notifications`, KDE Plasma and
//! GNOME): "<agent> wants to drive your desktop" with Allow, Deny, Postpone and Always allow
//! buttons. The tray menu offers the same choices, so a desktop without notification actions
//! still has a way to answer. Agent-supplied text goes in the body as plain text only.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// A button pressed on a drive prompt: the drive request id and `allow` | `deny` | `postpone` | `always_allow`;
/// or on a human step: the step id and `done` | `cancel`.
pub struct DriveAnswer(pub String, pub &'static str);

pub struct Notifier {
    proxy: Proxy<'static>,
    /// notification id -> drive request id
    shown: Arc<Mutex<HashMap<u32, String>>>,
}

fn decision(action: &str) -> Option<&'static str> {
    Some(match action {
        "allow" | "default" => "allow",
        "deny" => "deny",
        "postpone" => "postpone",
        "always_allow" => "always_allow",
        "done" => "done",
        "cancel" => "cancel",
        _ => return None,
    })
}

/// Notification bodies are markup on some servers; agent text must not become markup.
fn plain(s: &str) -> String {
    s.chars().take(200).collect::<String>().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Notifier {
    /// Connect and forward every button press to `tx` as `A::from(DriveAnswer)`.
    pub fn new<A>(tx: mpsc::Sender<A>) -> Result<Self, String>
    where
        A: From<DriveAnswer> + Send + 'static,
    {
        let conn = Connection::session().map_err(|e| e.to_string())?;
        let proxy = Proxy::new(&conn, DEST, PATH, DEST).map_err(|e| e.to_string())?;
        let shown: Arc<Mutex<HashMap<u32, String>>> = Arc::default();
        let it = proxy.receive_signal("ActionInvoked").map_err(|e| e.to_string())?;
        let map = shown.clone();
        std::thread::spawn(move || {
            for m in it {
                let Ok((id, action)) = m.body().deserialize::<(u32, String)>() else { continue };
                // Kept until `close`: a resident notification stays up until the broker says it was answered.
                let Some(drive) = map.lock().unwrap().get(&id).cloned() else { continue };
                if let Some(d) = decision(&action) {
                    let _ = tx.send(A::from(DriveAnswer(drive, d)));
                }
            }
        });
        Ok(Self { proxy, shown })
    }

    pub fn show(&self, drive: &str, agent_name: &str, description: &str) {
        let actions = ["allow", "Allow", "deny", "Deny", "postpone", "Postpone 10 min", "always_allow", "Always allow"];
        // Critical urgency stays on screen until answered; resident keeps it after a button press
        // until we close it on the broker's `drive_decided` (so a refused answer is not lost).
        let hints: HashMap<&str, Value> = HashMap::from([("urgency", Value::U8(2)), ("resident", Value::Bool(true)), ("desktop-entry", Value::from(crate::shortcut::APP_ID))]);
        let summary = format!("{} wants to drive your desktop", plain(agent_name));
        let body = if description.trim().is_empty() { "It will move the pointer and type until its task ends.".to_string() } else { format!("Task: {}", plain(description)) };
        let r: zbus::Result<u32> = self.proxy.call("Notify", &("Agentic Desktop Layer", 0u32, "input-mouse", summary, body, &actions[..], hints, 0i32));
        match r {
            Ok(id) => {
                self.shown.lock().unwrap().insert(id, drive.to_string());
            }
            Err(e) => eprintln!("w3bs-desktop tray: drive prompt notification failed ({e}); answer from the tray menu"),
        }
    }

    /// An agent handed the person a step (CAPTCHA, 2FA, sign-in): Done and Cancel buttons. The
    /// answer comes back as a `DriveAnswer` carrying the step id (`human_...`).
    pub fn show_human_step(&self, step: &str, agent_name: &str, kind: &str, message: &str) {
        let actions = ["done", "Done", "cancel", "Cancel"];
        let hints: HashMap<&str, Value> = HashMap::from([("urgency", Value::U8(2)), ("resident", Value::Bool(true)), ("desktop-entry", Value::from(crate::shortcut::APP_ID))]);
        let what = match kind {
            "captcha" => "a human check (CAPTCHA)",
            "two_factor" => "a sign-in code",
            "login" => "a sign-in",
            "payment" => "a payment confirmation",
            _ => "a step",
        };
        let summary = format!("{} needs you for {what}", plain(agent_name));
        let body = format!("{}\nAgent input is paused. Press Done when you have finished.", plain(message));
        let r: zbus::Result<u32> = self.proxy.call("Notify", &("Agentic Desktop Layer", 0u32, "dialog-information", summary, body, &actions[..], hints, 0i32));
        match r {
            Ok(id) => {
                self.shown.lock().unwrap().insert(id, step.to_string());
            }
            Err(e) => eprintln!("w3bs-desktop tray: human step notification failed ({e}); answer from the tray menu"),
        }
    }

    /// The request was answered (anywhere) or withdrawn: take its notification down.
    pub fn close(&self, drive: &str) {
        let ids: Vec<u32> = {
            let mut m = self.shown.lock().unwrap();
            let ids: Vec<u32> = m.iter().filter(|(_, d)| d.as_str() == drive).map(|(id, _)| *id).collect();
            for id in &ids {
                m.remove(id);
            }
            ids
        };
        for id in ids {
            let _: zbus::Result<()> = self.proxy.call("CloseNotification", &(id,));
        }
    }
}
