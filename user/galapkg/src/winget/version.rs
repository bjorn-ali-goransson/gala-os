//! winget's version ordering, for PackageVersion, a dependency's
//! MinimumVersion and an installed program's DisplayVersion.
//!
//! The rules (winget-cli's documentation of `winget upgrade`/`list` and
//! the manifest schema's PackageVersion; learn.microsoft.com "winget
//! upgrade"): a version is parts separated by `.`; each part is a leading
//! number and the text after it; parts compare by number, then by text
//! (any case), a part without text being the later one (`1.0` after
//! `1.0-beta`); missing parts count as `0`, so `1.2` equals `1.2.0`; a
//! leading `v` or `V` is ignored; `Unknown` (winget's name for a version
//! it could not read) is earlier than every other version. The ordering
//! itself is `crate::version` (the install path already uses it); this
//! module adds what the resolver needs on top, keeping the two from
//! drifting apart.
//!
//! Where the documentation is silent (gala's rules, deterministic): an
//! empty or blank version is `Unknown`; a part with no leading number
//! (`beta`) has number 0, so `1.beta` sorts before `1.0`; numbers too
//! long for 64 bits are the largest number.

use core::cmp::Ordering;

/// winget's ordering of two versions.
pub fn compare(a: &str, b: &str) -> Ordering {
    let unknown = |v: &str| v.trim().is_empty() || v.trim().eq_ignore_ascii_case("unknown");
    match (unknown(a), unknown(b)) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => crate::version::compare(a, b),
    }
}

/// Whether `version` satisfies a dependency's MinimumVersion (`None`:
/// any version does).
pub fn at_least(version: &str, minimum: Option<&str>) -> bool {
    minimum.is_none_or(|m| compare(version, m) != Ordering::Less)
}

/// The latest of `versions`; ties keep the first (deterministic).
pub fn latest<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    for v in versions {
        if best.is_none_or(|b| compare(v, b) == Ordering::Greater) {
            best = Some(v);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn documented_rules() {
        // Dotted parts compare as numbers, not text.
        assert_eq!(compare("1.2", "1.10"), Less);
        assert_eq!(compare("15.2.0", "15.10.0"), Less);
        // Missing parts are 0.
        assert_eq!(compare("1.2", "1.2.0"), Equal);
        assert_eq!(compare("1.2.0.0", "1.2"), Equal);
        assert_eq!(compare("1.0.0", "1.0.0.1"), Less);
        // A number with text after it is earlier than the bare number.
        assert_eq!(compare("1.0", "1.0-beta"), Greater);
        assert_eq!(compare("1.0-alpha", "1.0-beta"), Less);
        assert_eq!(compare("1.0RC", "1.0rc"), Equal);
        // A leading v is ignored.
        assert_eq!(compare("v2.0", "2.0"), Equal);
        assert_eq!(compare("V2.1", "2.0"), Greater);
        // Unknown is the earliest.
        assert_eq!(compare("Unknown", "0.0.1"), Less);
        assert_eq!(compare("unknown", "Unknown"), Equal);
        assert_eq!(compare("0", "unknown"), Greater);
    }

    #[test]
    fn gala_rules_where_silent() {
        assert_eq!(compare("", "0"), Less);
        assert_eq!(compare(" ", "unknown"), Equal);
        assert_eq!(compare("1.beta", "1.0"), Less);
        assert_eq!(compare("99999999999999999999999", "18446744073709551615"), Equal);
        assert_eq!(compare("2024.01.05", "2024.1.5"), Equal);
    }

    #[test]
    fn minimum_and_latest() {
        assert!(at_least("14.38.33130.0", Some("14.30")));
        assert!(!at_least("14.29", Some("14.30")));
        assert!(at_least("1.0", None));
        assert!(at_least("1.0", Some("1.0.0")));
        assert_eq!(latest(["1.0.0", "1.10.0", "1.9.9"]), Some("1.10.0"));
        assert_eq!(latest(["1.0", "1.0.0"]), Some("1.0"));
        assert_eq!(latest([]), None);
    }
}
