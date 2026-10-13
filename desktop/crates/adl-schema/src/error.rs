use serde::{Deserialize, Serialize};

/// Typed error codes shared by every interface (PRD section 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The layer is not Enabled.
    Disabled,
    /// Policy forbids this principal from doing this.
    Denied,
    /// A human must approve this exact request first; `approval_id` is in `details`.
    ApprovalRequired,
    /// The backend cannot do this on this platform/session.
    Unsupported,
    /// Focus, layout or a capture changed since the target was resolved.
    TargetStale,
    /// Another principal holds the input lease, or an approval is on screen.
    Busy,
    /// The action may or may not have taken effect; reconcile before retrying.
    OutcomeUnknown,
    /// No or invalid credentials.
    Unauthenticated,
    /// A required permission, backend, session or safety control is unavailable.
    Blocked,
    /// The work was cancelled by a stop, takeover or task cancel.
    Cancelled,
    /// Request failed schema or bounds validation.
    Invalid,
    /// Referenced task, window, display, approval or agent does not exist.
    NotFound,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Denied => "denied",
            Self::ApprovalRequired => "approval_required",
            Self::Unsupported => "unsupported",
            Self::TargetStale => "target_stale",
            Self::Busy => "busy",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Unauthenticated => "unauthenticated",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct AdlError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl AdlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), details: serde_json::Value::Null }
    }
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }
    pub fn disabled() -> Self {
        Self::new(ErrorCode::Disabled, "agent access is disabled")
    }
    pub fn denied(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Denied, msg)
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, msg)
    }
    pub fn stale(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::TargetStale, msg)
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, msg)
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, msg)
    }
}

pub type Result<T, E = AdlError> = std::result::Result<T, E>;
