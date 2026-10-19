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
/// against it and never substituted with another architecture or C library. A musl build
/// (Alpine, postmarketOS) asks for `*-unknown-linux-musl`: decided at compile time, which is
/// exact, since the running binary is the musl build.
pub fn host_platform() -> &'static str {
    let musl = cfg!(target_env = "musl");
    match (std::env::consts::ARCH, std::env::consts::OS, musl) {
        ("x86_64", "linux", false) => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux", false) => "aarch64-unknown-linux-gnu",
        ("x86_64", "linux", true) => "x86_64-unknown-linux-musl",
        ("aarch64", "linux", true) => "aarch64-unknown-linux-musl",
        ("x86_64", "macos", _) => "x86_64-apple-darwin",
        ("aarch64", "macos", _) => "aarch64-apple-darwin",
        _ => "unsupported",
    }
}

pub const SUPPORTED_PLATFORMS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-musl",
];

#[cfg(test)]
mod tests {
    #[test]
    fn host_platform_matches_the_build() {
        let p = super::host_platform();
        assert!(super::SUPPORTED_PLATFORMS.contains(&p), "{p}");
        if cfg!(target_os = "linux") {
            let env = if cfg!(target_env = "musl") { "musl" } else { "gnu" };
            assert_eq!(p, format!("{}-unknown-linux-{env}", std::env::consts::ARCH));
        }
    }
}
