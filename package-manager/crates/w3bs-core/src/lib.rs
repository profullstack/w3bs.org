//! Engine behind the `w3bs` CLI: packages (trust, resolution, staging, activation, receipts,
//! recovery) and W3BS resources (registry client with local verification).
//!
//! The reference implementation of the OpenPkg format (https://logicsrc.com/docs/openpkg).

pub mod archive;
pub mod db;
pub mod engine;
pub mod error;
pub mod fsutil;
pub mod layout;
pub mod lock;
pub mod repo;
pub mod resources;
pub mod transport;

pub use engine::repos_api::RepoAdd;
pub use engine::{Engine, InstallRequest, Interact, NoTerminal, Options, UpdateRequest};
pub use error::{Error, Kind, Result};
pub use layout::Layout;

pub fn now() -> String {
    jiff::Timestamp::now().round(jiff::Unit::Second).unwrap_or_else(|_| jiff::Timestamp::now()).to_string()
}
