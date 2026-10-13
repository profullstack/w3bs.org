//! Version selection policy.
//!
//! Default updates stay within the installed major version (within the minor for `0.x`).
//! `--allow-major` widens that boundary but never admits prereleases on a stable channel
//! and never overrides a pin. Nothing here ever selects a lower version.

use semver::Version;

/// Whether `candidate` is a compatible (non-breaking) upgrade of `installed`.
pub fn is_compatible(installed: &Version, candidate: &Version) -> bool {
    if installed.major == 0 {
        candidate.major == 0 && candidate.minor == installed.minor
    } else {
        candidate.major == installed.major
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection<'a> {
    /// Nothing newer exists.
    Current,
    /// A newer release exists, but only across a compatibility boundary.
    HeldMajor(&'a Version),
    /// Upgrade to this version.
    Upgrade(&'a Version),
}

/// Pick the best upgrade for `installed` from `available`.
///
/// `allow_prerelease` is true only on an explicitly selected prerelease channel.
pub fn select_upgrade<'a>(
    installed: &Version,
    available: impl IntoIterator<Item = &'a Version>,
    allow_major: bool,
    allow_prerelease: bool,
) -> Selection<'a> {
    let mut best_compatible: Option<&Version> = None;
    let mut best_any: Option<&Version> = None;
    for v in available {
        if v <= installed || (!allow_prerelease && !v.pre.is_empty()) {
            continue;
        }
        if best_any.is_none_or(|b| v > b) {
            best_any = Some(v);
        }
        if is_compatible(installed, v) && best_compatible.is_none_or(|b| v > b) {
            best_compatible = Some(v);
        }
    }
    match (best_compatible, best_any) {
        (_, Some(any)) if allow_major => Selection::Upgrade(any),
        (Some(c), _) => Selection::Upgrade(c),
        (None, Some(any)) => Selection::HeldMajor(any),
        (None, None) => Selection::Current,
    }
}

/// Latest release for a fresh install.
pub fn select_latest<'a>(available: impl IntoIterator<Item = &'a Version>, allow_prerelease: bool) -> Option<&'a Version> {
    available.into_iter().filter(|v| allow_prerelease || v.pre.is_empty()).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn compatible_upgrades() {
        let avail = [v("1.2.3"), v("1.2.4"), v("1.3.0"), v("2.0.0"), v("1.4.0-beta.1")];
        assert_eq!(select_upgrade(&v("1.2.3"), &avail, false, false), Selection::Upgrade(&avail[2]));
        assert_eq!(select_upgrade(&v("1.2.3"), &avail, true, false), Selection::Upgrade(&avail[3]));
        assert_eq!(select_upgrade(&v("1.3.0"), &avail, false, false), Selection::HeldMajor(&avail[3]));
        assert_eq!(select_upgrade(&v("2.0.0"), &avail, true, false), Selection::Current);
    }

    #[test]
    fn zero_x_minor_is_breaking() {
        let avail = [v("0.1.1"), v("0.2.0")];
        assert_eq!(select_upgrade(&v("0.1.0"), &avail, false, false), Selection::Upgrade(&avail[0]));
        assert_eq!(select_upgrade(&v("0.1.1"), &avail, false, false), Selection::HeldMajor(&avail[1]));
    }

    #[test]
    fn never_downgrades() {
        let avail = [v("1.0.0")];
        assert_eq!(select_upgrade(&v("1.0.1"), &avail, true, true), Selection::Current);
    }

    #[test]
    fn latest_skips_prereleases() {
        let avail = [v("1.0.0"), v("1.1.0-rc.1")];
        assert_eq!(select_latest(&avail, false), Some(&avail[0]));
        assert_eq!(select_latest(&avail, true), Some(&avail[1]));
    }
}
