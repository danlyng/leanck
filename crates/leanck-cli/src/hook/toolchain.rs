/// Only stable Lean releases at 4.29.0 or later (or later-version RCs) qualify.
pub(super) fn supports_push_not(toolchain_file_contents: &str) -> bool {
    let toolchain = toolchain_file_contents.trim();
    let version = toolchain
        .strip_prefix("leanprover/lean4:")
        .unwrap_or(toolchain);
    let version = version.strip_prefix('v').unwrap_or(version);
    let (version, rc) = match version.split_once("-rc") {
        Some((version, rc)) if !rc.is_empty() && rc.bytes().all(|c| c.is_ascii_digit()) => {
            (version, true)
        }
        Some(_) => return false,
        None => (version, false),
    };
    let mut parts = version.split('.');
    let mut numbers = [0_u64; 3];
    for number in &mut numbers {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
            return false;
        }
        let Ok(parsed) = part.parse() else {
            return false;
        };
        *number = parsed;
    }
    if parts.next().is_some() {
        return false;
    }
    numbers > [4, 29, 0] || (numbers == [4, 29, 0] && !rc)
}

#[cfg(test)]
mod tests {
    use super::supports_push_not;

    #[test]
    fn accepts_only_supported_release_versions() {
        for (toolchain, expected) in [
            ("leanprover/lean4:v4.28.1", false),
            ("leanprover/lean4:v4.29.0-rc8", false),
            ("leanprover/lean4:v4.29.0", true),
            ("v4.29.1", true),
            ("leanprover/lean4:v4.30.0-rc2", true),
            ("leanprover/lean4:v5.0.0", true),
            ("leanprover/lean4:nightly-2026-04-01", false),
            ("stable", false),
            (" \nleanprover/lean4:4.29.0\t", true),
            ("4.29.0", true),
            ("v4.29.1-rc1", true),
            ("v3.99.99", false),
            ("leanprover/lean4-nightly:nightly-2026-04-01", false),
            ("other/lean4:v4.29.0", false),
            ("", false),
            ("v4.29", false),
            ("v4.29.0.1", false),
            ("v4.29.0-rc", false),
            ("v4.29.0-rcx", false),
            ("v4.29.0-rc1-extra", false),
            ("v4.29.0-beta1", false),
            ("v4.29.0+build", false),
            ("v+4.29.0", false),
            ("v4..0", false),
            ("v4. 29.0", false),
            ("v999999999999999999999999.0.0", false),
        ] {
            assert_eq!(supports_push_not(toolchain), expected, "{toolchain}");
        }
    }
}
