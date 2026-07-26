//! Selecting a provisioned toolchain for a version pin treated as a MINIMUM.
//!
//! Swift is the only caller: `swift-tools-version` is a floor, not an exact
//! version, so a provisioned toolchain `>=` the pin may be reused instead of
//! provisioning the exact one. The host preflight and the in-container entrypoint
//! both call this over the same cache, so they agree on which toolchain runs.

/// A `major.minor.patch` version, with absent components read as `0` so `"6.0"`
/// and `"6.0.0"` compare equal. A prerelease suffix is ignored (kenn never
/// selects one), and anything with a non-numeric or fourth component fails to
/// parse — so a stray cache entry like `6.0.staging` is not mistaken for a
/// version. Mirrors `resolve::dotnet`'s `SdkVersion`, minus .NET's band digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version3 {
    major: u32,
    minor: u32,
    patch: u32,
}

fn parse(s: &str) -> Option<Version3> {
    let core = s.split('-').next()?;
    let mut parts = core.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    let patch: u32 = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None; // more than three components — not a version dir name
    }
    Some(Version3 {
        major,
        minor,
        patch,
    })
}

/// The provisioned version to use for `pin`, treating `pin` as a MINIMUM:
///
/// - the exact `pin` when it is provisioned — most faithful to the declaration;
/// - else the HIGHEST provisioned version `>=` `pin` (a newer toolchain builds an
///   older tools-version in the older language mode; cross-major is allowed);
/// - else `None` — nothing satisfies the minimum, so the caller provisions `pin`.
///
/// Unparseable `available` entries are ignored. An unparseable `pin` matches only
/// an exact string, so even a malformed pin still resolves to its own dir if
/// present.
#[must_use]
pub fn best_compatible(pin: &str, available: &[String]) -> Option<String> {
    // The exact declared version wins whenever it is present.
    if available.iter().any(|v| v == pin) {
        return Some(pin.to_string());
    }
    let want = parse(pin)?;
    available
        .iter()
        .filter_map(|v| parse(v).map(|parsed| (parsed, v)))
        .filter(|(parsed, _)| *parsed >= want)
        .max_by_key(|(parsed, _)| *parsed)
        .map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests {
    use super::best_compatible;

    fn v(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn exact_match_is_preferred_over_a_higher_version() {
        assert_eq!(
            best_compatible("6.0", &v(&["6.0", "6.3"])).as_deref(),
            Some("6.0")
        );
    }

    #[test]
    fn highest_at_or_above_the_minimum_is_chosen_when_no_exact() {
        assert_eq!(
            best_compatible("6.1", &v(&["6.0", "6.3"])).as_deref(),
            Some("6.3")
        );
        // Cross-major: a 6.x toolchain satisfies a 5.9 minimum.
        assert_eq!(best_compatible("5.9", &v(&["6.3"])).as_deref(), Some("6.3"));
    }

    #[test]
    fn nothing_at_or_above_the_minimum_yields_none() {
        assert_eq!(best_compatible("6.5", &v(&["6.0", "6.3"])), None);
        assert_eq!(best_compatible("6.0", &v(&[])), None);
    }

    #[test]
    fn a_component_count_mismatch_still_resolves_by_order() {
        // "6.0.0" and "6.0" are equal, satisfying each other's minimum.
        assert_eq!(
            best_compatible("6.0.0", &v(&["6.0"])).as_deref(),
            Some("6.0")
        );
        assert_eq!(
            best_compatible("6.0", &v(&["6.0.0"])).as_deref(),
            Some("6.0.0")
        );
    }

    #[test]
    fn unparseable_entries_are_ignored_but_an_exact_string_still_matches() {
        // A staging/lock leftover name is not a candidate…
        assert_eq!(
            best_compatible("6.0", &v(&["6.0.staging", "6.3"])).as_deref(),
            Some("6.3")
        );
        // …and a malformed pin only matches its own exact string.
        assert_eq!(
            best_compatible("weird", &v(&["weird", "6.3"])).as_deref(),
            Some("weird")
        );
        assert_eq!(best_compatible("weird", &v(&["6.3"])), None);
    }
}
