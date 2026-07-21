//! `resolve` tests for python. Shared `tarball`/`fetcher` helpers come from
//! the parent test module.

use super::super::*;
use super::{fetcher, tarball};
use crate::pin::Language;

const PYTHON_INDEX_JSON: &str = r#"{
      "cpython-3.12.13-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
        "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":13,
        "url":"https://x/cpython-3.12.13-x86_64-gnu.tar.gz","sha256":"p31213"},
      "cpython-3.12.9-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
        "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":9,
        "url":"https://x/cpython-3.12.9-x86_64-gnu.tar.gz","sha256":"p3129"},
      "cpython-3.12.50-linux-x86_64-gnu-debug": {"name":"cpython","os":"linux","libc":"gnu",
        "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":50,
        "url":"https://x/cpython-3.12.50-debug-full.tar.zst","sha256":"ZSTD"},
      "cpython-3.12.99-linux-x86_64-musl": {"name":"cpython","os":"linux","libc":"musl",
        "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":99,
        "url":"https://x/MUSL-MUST-NOT-BE-PICKED.tar.gz","sha256":"MUSL"},
      "cpython-3.13.5a1-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
        "arch":{"family":"x86_64"},"major":3,"minor":13,"patch":5,
        "url":"https://x/ALPHA-MUST-NOT-BE-PICKED.tar.gz","sha256":"ALPHA"},
      "cpython-3.13.1-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
        "arch":{"family":"x86_64"},"major":3,"minor":13,"patch":1,
        "url":"https://x/cpython-3.13.1-x86_64-gnu.tar.gz","sha256":"p3131"}
    }"#;

/// A partial pin selects the newest matching patch — `3.12` must not jump to
/// 3.13, and must not stop at 3.12.9.
#[test]
fn python_resolves_a_partial_pin_to_its_newest_patch() {
    let got = resolve(
        Language::Python,
        "3.12",
        ".python-version",
        None,
        Arch::X64,
        &fetcher(PYTHON_INDEX_JSON),
    )
    .expect("resolve");
    assert_eq!(got.version, "3.12.13");
    assert_eq!(tarball(&got).1, "p31213");
    // A newer `.tar.zst` debug build must not win: we unpack gzip only, and
    // choosing it fails as "invalid gzip header" long after the resolve.
    assert_ne!(tarball(&got).1, "ZSTD", "picked a zstd variant");
}

/// The python image is glibc. A musl asset would resolve cleanly and then
/// fail to exec — the same trap as the .NET RID.
///
/// Mutation-checked: dropping the `libc == "gnu"` filter picks MUSL.
#[test]
fn python_never_picks_a_musl_asset() {
    let got = resolve(
        Language::Python,
        "3.12.13",
        ".python-version",
        None,
        Arch::X64,
        &fetcher(PYTHON_INDEX_JSON),
    )
    .expect("resolve");
    assert_eq!(tarball(&got).1, "p31213");
    assert_ne!(tarball(&got).1, "MUSL", "picked the musl decoy");
}

/// uv's index carries prereleases whose numeric major/minor/patch are
/// IDENTICAL to a final's — `cpython-3.15.0b3` reports 3/15/0 — so only the
/// key distinguishes them. Comparing versions alone selected Python
/// 3.15.0a1 on a real unpinned repo, which then indexed 1202 defs without
/// complaint: a prerelease interpreter is not wrong enough to fail.
///
/// Mutation-checked: dropping the key filter picks the ALPHA decoy, which is
/// newer than every final in the fixture.
#[test]
fn python_never_picks_a_prerelease() {
    let got = resolve(
        Language::Python,
        LATEST,
        "<no pin file>",
        None,
        Arch::X64,
        &fetcher(PYTHON_INDEX_JSON),
    )
    .expect("resolve");
    assert_ne!(tarball(&got).1, "ALPHA", "picked a prerelease");
    assert_eq!(got.version, "3.13.1", "newest FINAL, not newest overall");
}

#[test]
fn python_reports_an_unmatched_pin_with_its_source() {
    let err = resolve(
        Language::Python,
        "3.99",
        "/ws/.python-version",
        None,
        Arch::X64,
        &fetcher(PYTHON_INDEX_JSON),
    )
    .expect_err("no such version");
    let msg = err.to_string();
    assert!(msg.contains("3.99"), "{msg}");
    assert!(msg.contains("/ws/.python-version"), "{msg}");
}
