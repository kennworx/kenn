//! `resolve` tests for dotnet. Shared `tarball`/`fetcher` helpers come from
//! the parent test module.

use super::super::*;
use super::tarball;
use crate::pin::Language;

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
