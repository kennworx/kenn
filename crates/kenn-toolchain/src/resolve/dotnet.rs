use super::common::{Arch, Install, ResolveError, Resolved, LATEST};

const DOTNET_INDEX: &str =
    "https://builds.dotnet.microsoft.com/dotnet/release-metadata/releases-index.json";

/// An SDK version decomposed as .NET means it: `major.minor.<band><patch>`,
/// where `9.0.308` is feature band 3, patch 8. The band lives in the hundreds
/// digit, which is why plain semver comparison gets .NET wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SdkVersion {
    major: u32,
    minor: u32,
    band: u32,
    patch: u32,
}

fn parse_sdk_version(s: &str) -> Option<SdkVersion> {
    // Ignore any prerelease suffix; we never select one.
    let core = s.split('-').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let third: u32 = parts.next()?.parse().ok()?;
    Some(SdkVersion {
        major,
        minor,
        band: third / 100,
        patch: third % 100,
    })
}

/// Whether `cand` may satisfy `want` under `policy`.
///
/// The default when `rollForward` is absent is `patch` — NOT `latestMajor`,
/// which only applies when no version is given at all. Getting this wrong is how
/// a repo pinning 9.0.308 silently ends up on an 10.x SDK, which is the exact
/// failure this whole change exists to remove.
#[expect(
    clippy::match_same_arms,
    reason = "a policy mapping table: `disable` and the unknown-policy fallback \
              share a body by intent, and merging them would hide that the \
              strict reading is a deliberate choice for unknown input"
)]
fn roll_forward_allows(policy: &str, want: SdkVersion, cand: SdkVersion) -> bool {
    match policy {
        "disable" => cand == want,
        "patch" | "latestPatch" => {
            (cand.major, cand.minor, cand.band) == (want.major, want.minor, want.band)
                && cand.patch >= want.patch
        }
        "feature" | "latestFeature" => {
            (cand.major, cand.minor) == (want.major, want.minor)
                && (cand.band, cand.patch) >= (want.band, want.patch)
        }
        "minor" | "latestMinor" => {
            cand.major == want.major
                && (cand.minor, cand.band, cand.patch) >= (want.minor, want.band, want.patch)
        }
        "major" | "latestMajor" => cand >= want,
        // An unknown policy is treated as the strictest reading rather than the
        // loosest: silently widening the search is how the wrong SDK gets picked.
        _ => cand == want,
    }
}

/// The `releases.json` URLs whose channel could hold a satisfying SDK. Each is
/// about a megabyte, so fetching every channel to search it would be wasteful —
/// and under `disable`/`patch` all but one are provably irrelevant.
fn reachable_channels(index: &serde_json::Value, policy: &str, want: SdkVersion) -> Vec<String> {
    index
        .get("releases-index")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let cv = e.get("channel-version")?.as_str()?;
            let mut it = cv.split('.');
            let major: u32 = it.next()?.parse().ok()?;
            let minor: u32 = it.next()?.parse().ok()?;
            // Reachable if the channel's highest conceivable SDK could satisfy
            // the policy — an upper bound, so no candidate is missed.
            let ceiling = SdkVersion {
                major,
                minor,
                band: 99,
                patch: 99,
            };
            let reachable = roll_forward_allows(policy, want, ceiling)
                || (major, minor) == (want.major, want.minor);
            reachable.then(|| e.get("releases.json")?.as_str().map(str::to_string))?
        })
        .collect()
}

/// The best SDK in one channel: `(version, url, hash)`.
fn best_sdk_in(
    channel: &serde_json::Value,
    policy: &str,
    want: SdkVersion,
    arch: Arch,
    want_prerelease: bool,
) -> Option<(SdkVersion, String, String)> {
    let mut best: Option<(SdkVersion, String, String)> = None;
    let releases = channel
        .get("releases")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for release in releases {
        // `sdks` is EVERY SDK in the release; `sdk` is only one of them.
        // Searching `sdk` alone misses 9.0.308, which ships in release 9.0.11
        // whose headline `sdk` is 9.0.112.
        let sdks = release
            .get("sdks")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for sdk in sdks {
            let Some(version_str) = sdk.get("version").and_then(serde_json::Value::as_str) else {
                continue;
            };
            // Never select a prerelease unless the pin itself names one. .NET's
            // own `allowPrerelease` defaults false outside Visual Studio, and an
            // unpinned workspace resolving to an SDK 11 PREVIEW gave
            // MSBuildLocator "no instances of MSBuild could be detected" —
            // a message that blames the machine for a choice the resolver made.
            if version_str.contains('-') && !want_prerelease {
                continue;
            }
            let Some(cand) = parse_sdk_version(version_str) else {
                continue;
            };
            if !roll_forward_allows(policy, want, cand) {
                continue;
            }
            let Some((url, hash)) = tarball_for(sdk, arch) else {
                continue;
            };
            merge_best_with(&mut best, policy, (cand, url, hash));
        }
    }
    best
}

/// The `.tar.gz` for `arch` in one SDK entry, with its published hash.
fn tarball_for(sdk: &serde_json::Value, arch: Arch) -> Option<(String, String)> {
    let file = sdk
        .get("files")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|f| {
            f.get("rid").and_then(serde_json::Value::as_str) == Some(arch.dotnet_rid())
                && f.get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|n| n.ends_with(".tar.gz"))
        })?;
    Some((
        file.get("url")?.as_str()?.to_string(),
        file.get("hash")?.as_str()?.to_string(),
    ))
}

/// Keep the better of two candidates: the LOWEST satisfying version for the
/// ordinary policies (roll forward as little as possible), the HIGHEST for the
/// `latest*` ones.
fn merge_best_with(
    best: &mut Option<(SdkVersion, String, String)>,
    policy: &str,
    cand: (SdkVersion, String, String),
) {
    let prefer_highest = policy.starts_with("latest");
    let better = best.as_ref().is_none_or(|(current, _, _)| {
        if prefer_highest {
            cand.0 > *current
        } else {
            cand.0 < *current
        }
    });
    if better {
        *best = Some(cand);
    }
}

/// Fold a channel's best into the running best, preserving whichever wins.
fn merge_best(
    best: &mut Option<(SdkVersion, String, String)>,
    policy: &str,
    candidate: Option<(SdkVersion, String, String)>,
) {
    if let Some(c) = candidate {
        // Across channels the policy decides, exactly as it does within one.
        // Hardcoding "prefer lower" here was right for a pinned workspace —
        // rolling forward further than needed is what picked 10.x for a 9.x pin
        // — but it silently broke the UNPINNED case, which resolves via
        // `latestMajor`: measured on a real repo with no global.json, it picked
        // SDK 5.0.408, whose runtime is too old to host the Roslyn BuildHost.
        merge_best_with(best, policy, c);
    }
}

pub(super) fn resolve_dotnet(
    pin: &str,
    pin_source: &str,
    roll_forward: Option<&str>,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    // An unpinned workspace takes whatever the newest supported channel
    // publishes; `major` scope makes every candidate acceptable, and the
    // `latest*` preference then picks the highest.
    let (pin, policy) = if pin == LATEST {
        ("0.0.0", "latestMajor")
    } else {
        (pin, roll_forward.unwrap_or("patch"))
    };
    let want_prerelease = pin.contains('-');
    let want = parse_sdk_version(pin).ok_or_else(|| ResolveError::NoMatch {
        language: "dotnet",
        pin: pin.to_string(),
        pin_source: pin_source.to_string(),
        detail: " — not a recognizable SDK version".to_string(),
    })?;

    let get_json = |url: &str| -> Result<serde_json::Value, ResolveError> {
        let body = fetch_text(url).map_err(|message| ResolveError::Metadata {
            language: "dotnet",
            url: url.to_string(),
            message,
        })?;
        serde_json::from_str(&body).map_err(|e| ResolveError::Metadata {
            language: "dotnet",
            url: url.to_string(),
            message: e.to_string(),
        })
    };

    // Only channels the policy could reach are worth fetching; each is ~1 MB.
    let index = get_json(DOTNET_INDEX)?;
    let mut best: Option<(SdkVersion, String, String)> = None;
    for channel_url in reachable_channels(&index, policy, want) {
        let channel = get_json(&channel_url)?;
        merge_best(
            &mut best,
            policy,
            best_sdk_in(&channel, policy, want, arch, want_prerelease),
        );
    }

    let Some((version, url, hash)) = best else {
        return Err(ResolveError::NoMatch {
            language: "dotnet",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(
                " under rollForward {policy:?} for {} — note latestMinor does NOT cross a major",
                arch.dotnet_rid()
            ),
        });
    };

    Ok(Resolved {
        version: format!(
            "{}.{}.{}{:02}",
            version.major, version.minor, version.band, version.patch
        ),
        install: Install::Tarball {
            url,
            // .NET publishes SHA-512, not SHA-256.
            digest_hex: hash,
            digest_is_sha512: true,
            // The SDK tarball unpacks straight into DOTNET_ROOT with no wrapper.
            strip_components: 0,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::Language;
    use crate::resolve::resolve;
    use crate::resolve::testutil::tarball;

    const DOTNET_INDEX_JSON: &str = r#"{"releases-index":[
          {"channel-version":"10.0","releases.json":"https://x/10.0/releases.json"},
          {"channel-version":"9.0","releases.json":"https://x/9.0/releases.json"}
        ]}"#;

    // 9.0.308 ships in RELEASE 9.0.11 alongside 9.0.307 — the release version
    // and the SDK version are different numbers, which is why `sdks[]` must be
    // searched rather than `sdk`.
    const DOTNET_9_JSON: &str = r#"{"releases":[
          {"release-version":"9.0.11","sdk":{"version":"9.0.112"},"sdks":[
            {"version":"9.0.308","files":[
              {"name":"dotnet-sdk-linux-arm64.tar.gz","rid":"linux-arm64",
               "url":"https://x/dotnet-sdk-9.0.308-linux-arm64.tar.gz","hash":"AAA512"},
              {"name":"dotnet-sdk-linux-musl-arm64.tar.gz","rid":"linux-musl-arm64",
               "url":"https://x/MUSL-MUST-NOT-BE-PICKED.tar.gz","hash":"MUSL"},
              {"name":"dotnet-sdk-linux-x64.tar.gz","rid":"linux-x64",
               "url":"https://x/dotnet-sdk-9.0.308-linux-x64.tar.gz","hash":"BBB512"},
              {"name":"dotnet-sdk-linux-musl-x64.tar.gz","rid":"linux-musl-x64",
               "url":"https://x/MUSL-MUST-NOT-BE-PICKED.tar.gz","hash":"MUSL"}]},
            {"version":"9.0.112","files":[
              {"name":"dotnet-sdk-linux-arm64.tar.gz","rid":"linux-arm64",
               "url":"https://x/dotnet-sdk-9.0.112-linux-arm64.tar.gz","hash":"CCC512"},
              {"name":"dotnet-sdk-linux-musl-arm64.tar.gz","rid":"linux-musl-arm64",
               "url":"https://x/MUSL-MUST-NOT-BE-PICKED.tar.gz","hash":"MUSL"}]}]},
          {"release-version":"9.0.12","sdk":{"version":"9.0.310"},"sdks":[
            {"version":"9.0.310","files":[
              {"name":"dotnet-sdk-linux-arm64.tar.gz","rid":"linux-arm64",
               "url":"https://x/dotnet-sdk-9.0.310-linux-arm64.tar.gz","hash":"DDD512"},
              {"name":"dotnet-sdk-linux-musl-arm64.tar.gz","rid":"linux-musl-arm64",
               "url":"https://x/MUSL-MUST-NOT-BE-PICKED.tar.gz","hash":"MUSL"}]}]}
        ]}"#;

    const DOTNET_10_JSON: &str = r#"{"releases":[
          {"release-version":"10.0.1","sdk":{"version":"10.0.302"},"sdks":[
            {"version":"11.0.100-preview.6","files":[
              {"name":"dotnet-sdk-linux-musl-arm64.tar.gz","rid":"linux-musl-arm64",
               "url":"https://x/PREVIEW-MUST-NOT-BE-PICKED.tar.gz","hash":"PREVIEW"}]},
            {"version":"10.0.302","files":[
              {"name":"dotnet-sdk-linux-arm64.tar.gz","rid":"linux-arm64",
               "url":"https://x/GLIBC-MUST-NOT-BE-PICKED.tar.gz","hash":"GLIBC"},
              {"name":"dotnet-sdk-linux-musl-arm64.tar.gz","rid":"linux-musl-arm64",
               "url":"https://x/dotnet-sdk-10.0.302-linux-arm64.tar.gz","hash":"EEE512"}]}]}
        ]}"#;

    fn dotnet_fetcher(url: &str) -> Result<String, String> {
        Ok(match url {
            u if u.contains("releases-index") => DOTNET_INDEX_JSON,
            u if u.contains("/9.0/") => DOTNET_9_JSON,
            u if u.contains("/10.0/") => DOTNET_10_JSON,
            other => return Err(format!("unexpected url {other}")),
        }
        .to_string())
    }

    /// THE regression this whole change exists for. A workspace pinning 9.0.308
    /// with `latestMinor` must resolve to a 9.x SDK — `latestMinor` does NOT
    /// cross a major, and picking 10.0.302 is what indexed zero files at exit 0.
    #[test]
    fn dotnet_latest_minor_does_not_cross_a_major() {
        let got = resolve(
            Language::Dotnet,
            "9.0.308",
            "/ws/global.json",
            Some("latestMinor"),
            Arch::Arm64,
            &dotnet_fetcher,
        )
        .expect("resolve");
        assert!(got.version.starts_with("9."), "got {}", got.version);
        assert!(
            !tarball(&got).0.contains("10.0"),
            "must not cross to 10.x: {}",
            tarball(&got).0
        );
        // .NET publishes SHA-512.
        assert!(tarball(&got).2, "dotnet hashes are sha512");
    }

    /// `sdks[]` is every SDK in the release; `sdk` is only one of them. 9.0.308
    /// lives in release 9.0.11 whose `sdk` is 9.0.112, so searching `sdk` alone
    /// would report the pin as unavailable.
    #[test]
    fn dotnet_finds_a_pin_that_is_not_the_releases_headline_sdk() {
        let got = resolve(
            Language::Dotnet,
            "9.0.308",
            "/ws/global.json",
            Some("disable"),
            Arch::Arm64,
            &dotnet_fetcher,
        )
        .expect("resolve");
        assert_eq!(got.version, "9.0.308");
        assert_eq!(tarball(&got).1, "AAA512");
    }

    /// Every image is noble, so the GLIBC build is the right one — and it is
    /// also .NET's default artifact.
    ///
    /// This assertion ran the other way when the C# image was alpine, and the
    /// mismatch was found by RUNNING it: a `dotnet` that exists on disk and
    /// fails to exec with a bare "not found", naming neither the file nor the
    /// reason. Unifying on one glibc distro removed the whole class.
    ///
    /// Mutation-checked: switching `dotnet_rid` to the musl RIDs resolves to
    /// MUSL-MUST-NOT-BE-PICKED and this fails.
    #[test]
    fn dotnet_picks_the_glibc_build_not_the_musl_one() {
        for arch in [Arch::Arm64, Arch::X64] {
            let got = resolve(
                Language::Dotnet,
                "9.0.308",
                "g",
                Some("disable"),
                arch,
                &dotnet_fetcher,
            )
            .expect("resolve");
            assert!(
                !tarball(&got).0.contains("musl"),
                "must select the glibc build for {arch:?}, got {}",
                tarball(&got).0
            );
            assert_ne!(
                tarball(&got).1,
                "GLIBC",
                "picked the glibc decoy for {arch:?}"
            );
        }
    }

    #[test]
    fn dotnet_selects_the_requested_architecture() {
        let got = resolve(
            Language::Dotnet,
            "9.0.308",
            "g",
            Some("disable"),
            Arch::X64,
            &dotnet_fetcher,
        )
        .expect("resolve");
        assert_eq!(tarball(&got).1, "BBB512");
        // The glibc build. `linux-x64` as a substring also matches
        // `linux-musl-x64`, so assert the musl marker is ABSENT instead.
        assert!(!tarball(&got).0.contains("musl"), "{}", tarball(&got).0);
    }

    /// The default when `rollForward` is absent is `patch` — NOT `latestMajor`.
    /// Defaulting to the loosest policy is how a 9.x pin lands on a 10.x SDK.
    /// An UNPINNED workspace resolves via `latestMajor`, and that must pick the
    /// NEWEST SDK across channels — not the oldest. Measured on a real repo with
    /// no global.json: the cross-channel merge hardcoded "prefer lower" and
    /// chose SDK 5.0.408, whose runtime cannot host the Roslyn `BuildHost`, so the
    /// index failed with a framework-not-found that named neither cause.
    #[test]
    fn dotnet_unpinned_takes_the_newest_channel_not_the_oldest() {
        let got = resolve(
            Language::Dotnet,
            LATEST,
            "<no pin file>",
            None,
            Arch::Arm64,
            &dotnet_fetcher,
        )
        .expect("resolve");
        assert!(
            got.version.starts_with("10."),
            "unpinned must take the newest channel, got {}",
            got.version
        );
        // …but NEVER a prerelease. .NET's allowPrerelease defaults false outside
        // Visual Studio, and a preview SDK gave MSBuildLocator "no instances of
        // MSBuild could be detected" — blaming the machine for the resolver's
        // choice.
        assert!(
            !got.version.contains('-'),
            "picked a prerelease: {}",
            got.version
        );
        assert_ne!(tarball(&got).1, "PREVIEW");
    }

    #[test]
    fn dotnet_defaults_to_patch_not_latest_major() {
        let got = resolve(
            Language::Dotnet,
            "9.0.308",
            "g",
            None,
            Arch::Arm64,
            &dotnet_fetcher,
        )
        .expect("resolve");
        assert!(
            got.version.starts_with("9.0.3"),
            "same band: {}",
            got.version
        );
    }

    /// `disable` means exactly that version, even when a newer one exists.
    #[test]
    fn dotnet_disable_refuses_to_roll_forward() {
        let err = resolve(
            Language::Dotnet,
            "9.0.399",
            "/ws/global.json",
            Some("disable"),
            Arch::Arm64,
            &dotnet_fetcher,
        )
        .expect_err("no exact match");
        let msg = err.to_string();
        assert!(msg.contains("9.0.399"), "{msg}");
        assert!(msg.contains("/ws/global.json"), "names the pin file: {msg}");
    }

    #[test]
    fn sdk_versions_decompose_band_and_patch() {
        let v = parse_sdk_version("9.0.308").expect("parse");
        assert_eq!((v.major, v.minor, v.band, v.patch), (9, 0, 3, 8));
        let v2 = parse_sdk_version("9.0.112").expect("parse");
        assert_eq!((v2.band, v2.patch), (1, 12));
        // Band ordering beats plain numeric ordering on the third component.
        assert!(parse_sdk_version("9.0.308") > parse_sdk_version("9.0.112"));
    }

    #[test]
    fn roll_forward_policies_scope_correctly() {
        let want = parse_sdk_version("9.0.308").unwrap();
        let same_band_newer = parse_sdk_version("9.0.310").unwrap();
        let other_band = parse_sdk_version("9.0.412").unwrap();
        let next_major = parse_sdk_version("10.0.302").unwrap();

        assert!(roll_forward_allows("patch", want, same_band_newer));
        assert!(!roll_forward_allows("patch", want, other_band));
        assert!(roll_forward_allows("feature", want, other_band));
        assert!(
            !roll_forward_allows("latestMinor", want, next_major),
            "must not cross a major"
        );
        assert!(roll_forward_allows("latestMajor", want, next_major));
        assert!(!roll_forward_allows("disable", want, same_band_newer));
        // An unknown policy is read strictly, not loosely.
        assert!(!roll_forward_allows("bogus", want, same_band_newer));
    }
}
