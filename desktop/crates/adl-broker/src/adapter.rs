//! The interface every desktop backend implements (PRD sections 9 and 13).
//!
//! Adapters are deliberately dumb: they report what exists and emit primitive events. All
//! authorization, leases, coordinate validation, chunking and cancellation live in the broker,
//! so a new platform cannot accidentally weaken the safety model.

use adl_schema::{AccessNode, Axis, Button, CapabilityReport, Key, Layout, Result, WindowInfo};

/// One primitive input event, in desktop logical coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    MotionAbsolute { x: f64, y: f64 },
    MotionRelative { dx: f64, dy: f64 },
    Button { button: Button, pressed: bool },
    Scroll { axis: Axis, steps: i32 },
    Key { key: Key, pressed: bool },
    /// Text insertion of a short chunk (at most `limits::TEXT_CHUNK_CHARS` characters).
    Text { text: String },
}

/// Something that happened to the session that the broker must react to.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Locked,
    Unlocked,
    Suspending,
    Resumed,
    LoggingOut,
    /// The human used a physical pointer or keyboard (only where reliably distinguishable).
    PhysicalInput,
    /// The emergency-stop shortcut fired.
    EmergencyStop,
    /// The OS revoked a permission the adapter depended on.
    PermissionLost(String),
    /// Display configuration changed; captures and display-space points are now stale.
    LayoutChanged,
    /// The adapter process or backend failed.
    AdapterFailed(String),
}

pub trait Adapter: Send + Sync {
    /// Stable backend identity, e.g. `mock`, `linux-portal`, `windows-uia`.
    fn name(&self) -> String;
    fn capabilities(&self) -> Vec<CapabilityReport>;
    /// Prepare backend sessions (portal sessions, permission prompts) when the human enables.
    fn begin(&self) -> Result<()>;
    /// Tear down every backend session and stream. Must not block on a hung application.
    fn end(&self);
    /// Close any open observation streams; returns how many were closed.
    fn close_streams(&self) -> usize {
        0
    }
    fn layout(&self) -> Result<Layout>;
    fn windows(&self) -> Result<Vec<WindowInfo>>;
    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        Ok(self.windows()?.into_iter().find(|w| w.focused))
    }
    /// Topmost window at a desktop logical point, used to detect occlusion before clicking.
    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>>;
    fn inspect(&self, window: &str) -> Result<AccessNode>;
    /// PNG of one display, already rotated as displayed. Returns (png, pixel width, pixel height).
    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)>;
    fn pointer_position(&self) -> Result<Option<(f64, f64)>> {
        Ok(None)
    }
    fn emit(&self, event: &InputEvent) -> Result<()>;
    /// Reason input must not be delivered right now (lock screen, an OS authentication prompt,
    /// a pointer lock). Checked before every action; `None` means clear.
    fn safety_interlock(&self) -> Option<String> {
        None
    }
    /// Drain session events. Called by the broker's supervisor loop.
    fn poll_events(&self) -> Vec<SessionEvent> {
        Vec::new()
    }
}
