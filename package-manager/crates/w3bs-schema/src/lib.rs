//! W3BS Package Manifest and Lifecycle Specification: data types shared by the
//! `w3bs` client and the `w3bs-publish` tooling.

pub mod identity;
pub mod manifest;
pub mod policy;
pub mod receipt;
pub mod result;

pub use identity::{MANAGER_PACKAGE_ID, PackageId, PackageRef};
pub use manifest::{ArtifactFormat, ChannelIndex, Manifest, PackageIndex};

/// The Rust target triple this binary was built for; artifacts are matched exactly
/// against it and never substituted with another architecture.
pub fn host_platform() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        _ => "unsupported",
    }
}

pub const SUPPORTED_PLATFORMS: &[&str] =
    &["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"];
