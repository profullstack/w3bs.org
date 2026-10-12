//! Installation receipts: exportable JSON views of committed state.
//! The SQLite database is the source of truth; receipts never describe an uncommitted install.

use crate::identity::PackageId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub schema_version: String,
    pub package_id: PackageId,
    pub owner: String,
    pub installed_version: String,
    pub platform: String,
    pub repository_id: String,
    pub root_fingerprint: String,
    pub artifact_sha256: String,
    pub installed_at: String,
    pub channel: String,
    pub update_policy: String,
    pub pinned_version: Option<String>,
    pub previous_version: Option<String>,
    pub commands: Vec<String>,
    pub transaction_id: String,
    /// Diagnostic only: which route created the installation (`native` or `curl`).
    /// Never changes ownership or security guarantees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}
