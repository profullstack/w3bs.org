//! Local IPC framing: one JSON object per line over a Unix-domain socket.
//!
//! Two sockets, two vocabularies. The *agent* socket requires a bearer token in `hello` and never
//! exposes enable, grant or approval methods. The *control* socket is for the human's native
//! control and CLI; it has no agent identity and refuses peers descended from agent clients.

use crate::error::AdlError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AdlError>,
}

impl Response {
    pub fn ok(id: String, result: serde_json::Value) -> Self {
        Self { v: crate::SCHEMA_VERSION, id, result: Some(result), error: None }
    }
    pub fn err(id: String, error: AdlError) -> Self {
        Self { v: crate::SCHEMA_VERSION, id, result: None, error: Some(error) }
    }
}

/// Server-pushed event on a subscribed control connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub v: u32,
    pub event: String,
    pub data: serde_json::Value,
}

/// Methods on the agent socket. Anything else is `denied`.
pub const AGENT_METHODS: &[&str] = &[
    "hello", "status", "capabilities", "layout", "windows", "inspect", "screenshot", "pointer.position",
    "task.begin", "task.end", "task.cancel", "task.list", "task.inspect", "lease.acquire", "lease.renew",
    "lease.release", "input", "approval.status", "human_step.request", "human_step.status",
    "memory.remember", "memory.locate", "memory.list",
    "browser.status", "browser.pages", "browser.snapshot", "browser.screenshot", "browser.wait_for", "browser.act",
];

/// Methods on the control socket.
pub const CONTROL_METHODS: &[&str] = &[
    "status", "capabilities", "doctor", "enable.request", "enable.confirm", "disable", "stop", "takeover",
    "agents.add", "agents.list", "agents.update", "agents.revoke", "agents.remove", "tasks.list",
    "tasks.inspect", "tasks.cancel", "approvals.list", "approvals.decide", "drive.list", "drive.decide",
    "drive.always_allow", "human_steps.list", "human_steps.resolve",
    "memory.all", "memory.forget", "audit.export", "audit.delete",
    "audit.recent", "session.event", "subscribe", "control.login", "native.shortcut",
    "training.status", "training.apps", "training.start", "training.pause", "training.resume", "training.mark",
    "training.explain", "training.finish", "training.discard", "training.forget", "training.handoff",
    "browser.probe",
];
