//! Release compatibility, independent of executable paths and filesystem identity.
use semver::Version;

pub(super) fn check(version: Option<&str>) -> Option<bool> {
    compatible(version?, env!("CARGO_PKG_VERSION"))
}

fn compatible(daemon: &str, client: &str) -> Option<bool> {
    let parse = |text: &str| Version::parse(text.strip_prefix('v').unwrap_or(text)).ok();
    let daemon = parse(daemon)?;
    let client = parse(client)?;
    if !daemon.pre.is_empty() || !client.pre.is_empty() {
        // Prereleases make no compatibility promise; build metadata is immaterial.
        return Some(daemon.cmp_precedence(&client).is_eq());
    }
    Some(
        daemon.major == client.major
            && (client.major != 0
                || (daemon.minor == client.minor
                    && (client.minor != 0 || daemon.patch == client.patch))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_versions_follow_semver_compatibility_without_comparing_strings() {
        for (daemon, client, expected) in [
            ("1.9.0", "1.10.0", true),
            ("2.0.0", "1.9.0", false),
            ("0.40.0", "0.40.2", true),
            ("0.39.99", "0.40.0", false),
            ("0.0.1", "0.0.2", false),
            ("v0.40.0+build.1", "0.40.0+build.2", true),
            ("0.40.0-rc.1", "0.40.0", false),
            ("0.40.0-rc.1", "0.40.0-rc.2", false),
            ("0.40.0-rc.1+first", "0.40.0-rc.1+second", true),
        ] {
            assert_eq!(
                compatible(daemon, client),
                Some(expected),
                "{daemon} vs {client}"
            );
            assert_eq!(
                compatible(client, daemon),
                Some(expected),
                "{client} vs {daemon}"
            );
        }
        for version in ["", "development", "0.40", "01.40.0"] {
            assert_eq!(compatible(version, "0.40.0"), None);
        }
        assert_eq!(check(None), None);
    }
}
