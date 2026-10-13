//! windows: native backend (port in progress).

use crate::AdapterKind;
use adl_broker::Adapter;
use adl_schema::*;
use std::sync::Arc;

pub fn make_adapter(kind: AdapterKind) -> Result<Arc<dyn Adapter>> {
    match kind {
        AdapterKind::Mock => Ok(Arc::new(adl_broker::mock::MockDesktop::new())),
        AdapterKind::Auto | AdapterKind::Native => Err(AdlError::new(ErrorCode::Blocked, "the windows native backend is not implemented yet")),
        AdapterKind::Vela | AdapterKind::Portal => Err(AdlError::unsupported("vela and portal are Linux backends")),
    }
}

pub fn run_tray() -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "the windows native control is not implemented yet"))
}
