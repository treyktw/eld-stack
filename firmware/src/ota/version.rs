//! Firmware version parsing and comparison.
//!
//! Pure — no esp-idf, no allocation — so the host harness can exercise it.
//!
//! Versions are `MAJOR.MINOR.PATCH`, all decimal, no pre-release suffix. The
//! narrow grammar is deliberate: the version decides whether a truck installs
//! new code, and every extra accepted form is another way for two builds to
//! compare in a way nobody intended.

/// A parsed `MAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse `"1.2.3"`.
    ///
    /// Rejects anything else: leading `v`, extra components, empty fields,
    /// signs, whitespace, and leading zeros. Leading zeros are refused because
    /// `01` and `1` would otherwise be two spellings of one version, and the
    /// update decision would depend on which spelling the server happened to
    /// send.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let major = parse_component(parts.next()?)?;
        let minor = parse_component(parts.next()?)?;
        let patch = parse_component(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

fn parse_component(text: &str) -> Option<u16> {
    if text.is_empty() || text.len() > 5 {
        return None;
    }
    if text.len() > 1 && text.as_bytes()[0] == b'0' {
        return None;
    }

    let mut value: u16 = 0;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u16)?;
    }
    Some(value)
}

/// Whether `candidate` should replace `current`.
///
/// Strictly newer only. Equal is not an update — reinstalling the running build
/// costs a reboot and cellular data for nothing, and a server stuck republishing
/// the same manifest would otherwise put a truck in a reboot loop.
///
/// Downgrades are refused here on purpose. Rolling back is a real need, but it
/// is a deliberate act with different safety rules (the bootloader's own
/// rollback, or an explicitly forced install), not something a routine manifest
/// poll should be able to trigger. A server that has been tampered with should
/// not be able to push a truck back onto a known-vulnerable build.
pub fn should_install(current: Version, candidate: Version) -> bool {
    candidate > current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_versions() {
        assert_eq!(Version::parse("0.1.7"), Some(Version::new(0, 1, 7)));
        assert_eq!(Version::parse("10.20.30"), Some(Version::new(10, 20, 30)));
        assert_eq!(Version::parse("0.0.0"), Some(Version::new(0, 0, 0)));
    }

    #[test]
    fn rejects_malformed_versions() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.",
            ".1.2",
            "1..2",
            "1.2.x",
            " 1.2.3",
            "1.2.3 ",
            "-1.2.3",
            "1.2.-3",
            "01.2.3",
            "1.02.3",
            "999999.0.0",
        ] {
            assert_eq!(Version::parse(bad), None, "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn orders_by_component_significance() {
        assert!(Version::new(1, 0, 0) > Version::new(0, 99, 99));
        assert!(Version::new(0, 2, 0) > Version::new(0, 1, 99));
        assert!(Version::new(0, 1, 8) > Version::new(0, 1, 7));
    }

    #[test]
    fn installs_only_strictly_newer() {
        let current = Version::new(0, 1, 7);
        assert!(should_install(current, Version::new(0, 1, 8)));
        assert!(should_install(current, Version::new(0, 2, 0)));
        assert!(should_install(current, Version::new(1, 0, 0)));
    }

    #[test]
    fn refuses_same_version() {
        // A server republishing the manifest must not put a truck in a reboot
        // loop.
        let current = Version::new(0, 1, 7);
        assert!(!should_install(current, current));
    }

    #[test]
    fn refuses_downgrade() {
        // A tampered manifest must not be able to push a truck back onto a
        // known-vulnerable build.
        let current = Version::new(0, 2, 0);
        assert!(!should_install(current, Version::new(0, 1, 9)));
        assert!(!should_install(current, Version::new(0, 0, 1)));
    }
}
