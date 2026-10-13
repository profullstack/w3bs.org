//! Windows: the native adapter (SendInput, UI Automation, GDI capture, WTS session events) and
//! the notification-area native control.

use crate::AdapterKind;
use adl_broker::Adapter;
use adl_schema::*;
use std::sync::Arc;

#[path = "windows_tray.rs"]
mod tray;

pub fn make_adapter(kind: AdapterKind) -> Result<Arc<dyn Adapter>> {
    match kind {
        AdapterKind::Mock => Ok(Arc::new(adl_broker::mock::MockDesktop::new())),
        AdapterKind::Auto | AdapterKind::Native => {
            // SESSIONNAME is unset for services (session 0), which have no interactive desktop.
            if std::env::var_os("SESSIONNAME").is_none() && std::env::var_os("W3BS_DESKTOP_ALLOW_NO_SESSIONNAME").is_none() {
                eprintln!("w3bs-desktop: SESSIONNAME is not set; if this is a service there is no interactive desktop to drive");
            }
            Ok(Arc::new(adl_windows::WindowsAdapter::new()))
        }
        AdapterKind::Vela | AdapterKind::Portal => Err(AdlError::unsupported("vela and portal are Linux backends; on Windows use auto or native")),
    }
}

pub fn run_tray() -> std::io::Result<()> {
    tray::run()
}
