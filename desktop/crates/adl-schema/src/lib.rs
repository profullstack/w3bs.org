//! Agentic Desktop Layer schemas, shared by the broker, adapters, CLI and MCP surface.
//!
//! Every wire object carries `SCHEMA_VERSION`. Breaking changes bump it; additive fields do not.

pub mod coords;
pub mod error;
pub mod input;
pub mod keys;
pub mod model;
pub mod rpc;

pub use coords::{DisplayInfo, Point, Rect, Rotation, Space};
pub use error::{AdlError, ErrorCode, Result};
pub use input::{Action, Axis, Button, InputBatch, Target};
pub use keys::{Key, KeyCombo, Modifier};
pub use model::*;

/// Version of every schema in this crate.
pub const SCHEMA_VERSION: u32 = 1;

/// Hard bounds the broker enforces on any single input batch (PRD section 7).
pub mod limits {
    pub const MAX_ACTIONS_PER_BATCH: usize = 256;
    pub const MAX_TEXT_CHARS: usize = 4096;
    /// Text is delivered in chunks of this many characters with a focus check between chunks.
    pub const TEXT_CHUNK_CHARS: usize = 32;
    pub const MAX_HOLD_MS: u64 = 5_000;
    pub const MAX_HOVER_MS: u64 = 10_000;
    pub const MAX_DRAG_STEPS: u32 = 200;
    pub const MAX_SCROLL_STEPS: i32 = 100;
    pub const MAX_CLICK_COUNT: u8 = 3;
    pub const MAX_SHORTCUT_KEYS: usize = 6;
    pub const MAX_LEASE_TTL_MS: u64 = 10 * 60 * 1000;
    pub const DEFAULT_LEASE_TTL_MS: u64 = 60 * 1000;
}
