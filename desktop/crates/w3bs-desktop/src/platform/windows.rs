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
            // Services run in session 0, which has no interactive desktop to drive.
            if adl_windows::win::current_session() == Some(0) {
                eprintln!("w3bs-desktop: running in session 0 (a service); there is no interactive desktop here");
            }
            Ok(Arc::new(adl_windows::WindowsAdapter::new()))
        }
        AdapterKind::Vela | AdapterKind::Portal => Err(AdlError::unsupported("vela and portal are Linux backends; on Windows use auto or native")),
    }
}

pub fn run_tray() -> std::io::Result<()> {
    tray::run()
}
