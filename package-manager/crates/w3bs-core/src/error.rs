use w3bs_schema::result::{ErrorInfo, Overall};

/// How an error maps onto the exit-code contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Invalid arguments, ambiguous identity, invalid configuration (exit 2).
    Usage,
    /// Approval or a trust decision is required (exit 3).
    Approval,
    /// Global security/state/environment preflight failure (exit 4).
    Preflight,
    /// Mutation lock unavailable (exit 5).
    Locked,
    /// A per-package failure (exit 1).
    Package,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub kind: Kind,
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(kind: Kind, code: &'static str, message: impl Into<String>) -> Self {
        Self { kind, code, message: message.into(), retryable: false }
    }
    pub fn usage(code: &'static str, m: impl Into<String>) -> Self {
        Self::new(Kind::Usage, code, m)
    }
    pub fn approval(code: &'static str, m: impl Into<String>) -> Self {
        Self::new(Kind::Approval, code, m)
    }
    pub fn preflight(code: &'static str, m: impl Into<String>) -> Self {
        Self::new(Kind::Preflight, code, m)
    }
    pub fn package(code: &'static str, m: impl Into<String>) -> Self {
        Self::new(Kind::Package, code, m)
    }
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
    pub fn info(&self) -> ErrorInfo {
        ErrorInfo { code: self.code.to_owned(), message: self.message.clone(), retryable: self.retryable }
    }
    pub fn overall(&self) -> Overall {
        match self.kind {
            Kind::Usage => Overall::InvalidArguments,
            Kind::Approval => Overall::ApprovalRequired,
            Kind::Preflight => Overall::PreflightFailure,
            Kind::Locked => Overall::Locked,
            Kind::Package => Overall::Failure,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::preflight("STATE_DATABASE", format!("state database error: {e}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::package("IO", e.to_string())
    }
}

/// Attach context to io errors.
pub trait IoContext<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| Error::package("IO", format!("{}: {e}", what())))
    }
}
