//! Linux: Vela command socket, or XDG portals with KWin / GNOME Shell window data.

use crate::paths;
use crate::AdapterKind;
use adl_broker::Adapter;
use adl_schema::*;
use std::sync::Arc;

pub fn make_adapter(kind: AdapterKind) -> Result<Arc<dyn Adapter>> {
    use adl_linux::gnome::{self, ShellBus, ShellObserver, WithGnome};
    use adl_linux::kwin::{KWinObserver, KWinScripts, WithKWin};
    use adl_linux::{portal::PortalAdapter, vela::VelaAdapter};
    let vela = || VelaAdapter::default_socket().map(VelaAdapter::new).ok_or_else(|| AdlError::new(ErrorCode::Blocked, "Vela command socket not found (needs XDG_RUNTIME_DIR and WAYLAND_DISPLAY)"));
    // The portal backend gets its window list, focus and stacking from KWin on Plasma and from
    // the W3BS Shell extension on GNOME.
    let portal = || -> Arc<dyn Adapter> {
        let p = PortalAdapter::new(paths::config_dir().join("portal-restore-token"));
        if KWinObserver::detect() {
            return match KWinScripts::connect() {
                Ok(src) => Arc::new(WithKWin::new(p, KWinObserver::new(Box::new(src)))),
                Err(e) => {
                    eprintln!("w3bs-desktop: KWin window observer unavailable ({}); targeted input will be refused", e.message);
                    Arc::new(p.with_windows_note(format!("the KWin window observer is unavailable ({}); targeted input is refused", e.message)))
                }
            };
        }
        if gnome::detect() {
            return match ShellBus::connect() {
                Ok(src) => Arc::new(WithGnome::new(p, ShellObserver::new(Box::new(src)))),
                Err(e) => {
                    eprintln!("w3bs-desktop: no GNOME Shell window data ({}); targeted input will be refused", e.message);
                    Arc::new(p.with_windows_note(format!("GNOME Shell window data is unavailable: {}; targeted input is refused until then (restart the daemon after installing)", e.message)))
                }
            };
        }
        Arc::new(p)
    };
    Ok(match kind {
        AdapterKind::Mock => Arc::new(adl_broker::mock::MockDesktop::new()),
        AdapterKind::Native => return Err(AdlError::unsupported("`native` is the Windows/macOS backend; on Linux use auto, portal or vela")),
        AdapterKind::Vela => Arc::new(vela()?),
        AdapterKind::Portal => portal(),
        AdapterKind::Auto if VelaAdapter::detect() => Arc::new(vela()?),
        AdapterKind::Auto if PortalAdapter::detect() => portal(),
        AdapterKind::Auto => return Err(AdlError::new(ErrorCode::Blocked, "no supported desktop session found (need a Wayland/X11 session with D-Bus, or Vela); use --adapter mock for a demo")),
    })
}

pub fn run_tray() -> std::io::Result<()> {
    crate::tray::run()
}
