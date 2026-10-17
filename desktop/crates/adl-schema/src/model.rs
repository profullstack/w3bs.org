//! Global state, capabilities, observation results, tasks, grants and operation results.

use crate::coords::{CaptureMeta, DisplayInfo, Rect};
use crate::error::AdlError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Global layer state (PRD section 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalState {
    Disabled,
    Enabling,
    Enabled,
    Stopping,
    Blocked,
}

impl GlobalState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Enabling => "Enabling",
            Self::Enabled => "Enabled",
            Self::Stopping => "Stopping",
            Self::Blocked => "Blocked",
        }
    }
}

/// Separately visible permissions (PRD section 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Pointer,
    Keyboard,
    ScreenCapture,
    Windows,
    Accessibility,
    ClipboardRead,
    ClipboardWrite,
    Browser,
    Files,
    Email,
    Calendar,
}

impl Capability {
    pub const ALL: [Capability; 11] = [
        Self::Pointer, Self::Keyboard, Self::ScreenCapture, Self::Windows, Self::Accessibility, Self::ClipboardRead,
        Self::ClipboardWrite, Self::Browser, Self::Files, Self::Email, Self::Calendar,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    PermissionRequired,
    Unsupported,
    Degraded,
}

/// One adapter's report for one capability (PRD section 13).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub capability: Capability,
    pub availability: Availability,
    pub backend: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Finer-grained features, e.g. `relative_motion`, `pointer_position`, `button.back`,
    /// `target_bound_keyboard`, `physical_input_detection`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub features: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: String,
    pub app_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub generation: u64,
    pub displays: Vec<DisplayInfo>,
}

/// Accessible element (AT-SPI / UI Automation / AX). Handles expire with the observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccessNode {
    pub handle: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AccessNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation<T> {
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub layout_generation: u64,
    pub backend: String,
    pub data: T,
}

/// Screenshot result. The PNG is returned base64 on the wire; the CLI writes it to a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Screenshot {
    #[serde(flatten)]
    pub meta: CaptureMeta,
    pub png_base64: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Completed,
    Cancelled,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub agent: String,
    pub description: String,
    pub state: TaskState,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    pub batches_completed: u64,
    pub batches_cancelled: u64,
    pub batches_unknown: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeaseInfo {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub expires_at_ms: u64,
    pub generation: u64,
}

/// How much of the desktop an agent may drive (PRD section 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// Only explicit adapter actions on allowlisted windows; no raw coordinates outside them.
    #[default]
    Restricted,
    /// Visual/accessibility and input control of the whole desktop. Business-intent limits
    /// cannot be enforced in this mode, and the UI says so.
    BroadDesktop,
}

/// A per-agent grant. Default is nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Policy {
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub mode: ControlMode,
    /// App ids (exact, or prefix ending in `*`) the agent may target in restricted mode.
    #[serde(default)]
    pub apps: Vec<String>,
    /// Action kinds (see `Action::kind`) or operations that need per-request human approval.
    #[serde(default)]
    pub approval_required_for: BTreeSet<String>,
    /// Upper bound on input actions per minute; 0 = default limit.
    #[serde(default)]
    pub max_actions_per_minute: u32,
    /// Grant expiry; `None` = until revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
}

impl Policy {
    pub fn allows_app(&self, app_id: &str) -> bool {
        self.mode == ControlMode::BroadDesktop
            || self.apps.iter().any(|p| match p.strip_suffix('*') {
                Some(prefix) => app_id.starts_with(prefix),
                None => p == app_id,
            })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub policy: Policy,
    pub created_at_ms: u64,
    pub revoked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Consumed,
}

/// A request for a human decision, bound to exactly one operation (PRD section 11).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalInfo {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub operation: String,
    pub target: String,
    /// SHA-256 of the canonical request arguments. Any change invalidates the approval.
    pub args_digest: String,
    /// Human-readable summary with no typed text or message bodies.
    pub summary: String,
    pub status: ApprovalStatus,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveStatus {
    /// Waiting for the person at the desktop.
    Pending,
    /// The agent may drive until `until_ms`, which moves on each time it takes or renews the lease.
    Allowed,
    /// Refused until `until_ms` (`lease.acquire` answers `denied`); then it may ask again.
    Denied,
    /// Asked again after `until_ms`; until then `lease.acquire` answers `busy`.
    Postponed,
    Expired,
}

/// A person's answer to "this agent wants to drive your desktop".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveDecision {
    Allow,
    Deny,
    Postpone,
    /// Allow this and every later request without asking, until turned off (for when nobody is at the desk).
    AlwaysAllow,
}

/// An agent asking to take the input lease, shown to the person before it gets the desktop. One per
/// agent: the answer covers every task it runs while the answer lasts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriveRequest {
    pub id: String,
    pub agent: String,
    /// The agent's display name.
    pub agent_name: String,
    /// The task it is asking for (the latest, if it asked again).
    pub task: String,
    /// The task's own description (agent-supplied text; render as text only).
    pub description: String,
    pub status: DriveStatus,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// Resulting state was read back and matched.
    Verified,
    /// Events were accepted by the OS; nobody checked the effect.
    Dispatched,
    NotVerified,
}

/// Result of one input batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchResult {
    pub request: String,
    pub task: String,
    pub outcome: Outcome,
    pub verification: Verification,
    pub backend: String,
    /// Actions fully delivered before the batch ended.
    pub actions_completed: usize,
    pub actions_total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AdlError>,
}

/// What a stop/disable did (PRD section 7: report completed, cancelled and unknown separately).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct StopReport {
    pub reason: String,
    pub leases_revoked: usize,
    pub tasks_cancelled: usize,
    pub batches_interrupted: usize,
    pub approvals_withdrawn: usize,
    pub streams_closed: usize,
    pub synthetic_inputs_released: Vec<String>,
    /// Milliseconds from receipt until the gate rejected new work.
    pub gate_closed_ms: f64,
    pub cleanup_ms: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleanup_failures: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub schema: u32,
    pub state: GlobalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseInfo>,
    pub running_tasks: Vec<TaskInfo>,
    pub pending_approvals: usize,
    /// Agents waiting for the person to let them drive.
    #[serde(default)]
    pub pending_drive_requests: usize,
    /// Drive requests are allowed without asking (the person is away).
    #[serde(default)]
    pub drive_always_allow: bool,
    pub agents: usize,
    pub version: String,
}
