//! The machine interface: one versioned result object per invocation on stdout.

use serde::{Deserialize, Serialize};

pub const RESULT_SCHEMA_VERSION: &str = "1.0";

/// Process exit codes (PRD section 6).
pub mod exit {
    pub const OK: i32 = 0;
    pub const PACKAGE_FAILURE: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const APPROVAL_REQUIRED: i32 = 3;
    pub const PREFLIGHT: i32 = 4;
    pub const LOCKED: i32 = 5;
    pub const INTERRUPTED: i32 = 130;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Overall {
    Ok,
    PartialFailure,
    Failure,
    ApprovalRequired,
    InvalidArguments,
    PreflightFailure,
    Locked,
}

impl Overall {
    pub fn exit_code(self) -> i32 {
        match self {
            Overall::Ok => exit::OK,
            Overall::PartialFailure | Overall::Failure => exit::PACKAGE_FAILURE,
            Overall::ApprovalRequired => exit::APPROVAL_REQUIRED,
            Overall::InvalidArguments => exit::USAGE,
            Overall::PreflightFailure => exit::PREFLIGHT,
            Overall::Locked => exit::LOCKED,
        }
    }
}

/// Per-package outcome.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Installed,
    Updated,
    Removed,
    Current,
    Pinned,
    Unpinned,
    Held,
    Blocked,
    Failed,
    Unknown,
    Planned,
    RolledBack,
    Skipped,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageOutcome {
    pub id: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    /// Curl-installed tools: `signed by SHA256:…` when a W3BS-SIGN-1 publisher signature
    /// covered the installer that ran, `unsigned` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResult {
    pub schema_version: String,
    pub operation: String,
    pub result: Overall,
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageOutcome>,
    /// Operation-specific payload (inventory, search hits, repository info...).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl OperationResult {
    pub fn new(operation: &str) -> Self {
        Self {
            schema_version: RESULT_SCHEMA_VERSION.into(),
            operation: operation.into(),
            result: Overall::Ok,
            transaction_id: None,
            packages: vec![],
            data: serde_json::Value::Null,
            error: None,
            warnings: vec![],
        }
    }

    /// Derive the overall result from package outcomes.
    pub fn settle(&mut self) {
        if self.error.is_some() {
            return;
        }
        let failed = self.packages.iter().filter(|p| matches!(p.status, Status::Failed | Status::Conflict | Status::Blocked)).count();
        let good = self.packages.len() - failed;
        self.result = match (failed, good) {
            (0, _) => Overall::Ok,
            (_, 0) => Overall::Failure,
            _ => Overall::PartialFailure,
        };
    }
}
