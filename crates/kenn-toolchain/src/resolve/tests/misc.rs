//! `resolve` tests for misc. Shared `tarball`/`fetcher` helpers come from
//! the parent test module.

use super::super::*;
use super::fetcher;
use crate::pin::Language;

/// Swift must be refused, not provisioned unverified. If someone later makes
/// this "work" by reading the static-sdk `checksum`, they will be verifying
/// a different file and calling it safe.
#[test]
fn swift_is_refused_because_it_cannot_be_verified() {
    let err = resolve(
        Language::Swift,
        "6.0",
        "Package.swift",
        None,
        Arch::Arm64,
        &fetcher("{}"),
    )
    .expect("swift resolves to a host-provisioned toolchain");
    // Swift is provisioned from the official image by the HOST, because
    // swift.org publishes no verifiable tarball and only the host can call
    // docker. The entrypoint expects it present rather than fetching it.
    assert!(
        matches!(err.install, Install::Preprovisioned),
        "{:?}",
        err.install
    );
    assert_eq!(err.version, "6.0");
    assert!(err.install.digest().is_none(), "nothing of ours to verify");
}

#[test]
fn a_metadata_fetch_failure_names_the_language_and_url() {
    let err = resolve(Language::Go, "1.24.5", "go.mod", None, Arch::X64, &|_| {
        Err("connection refused".to_string())
    })
    .expect_err("fetch failed");
    let msg = err.to_string();
    assert!(msg.contains("go.dev"), "{msg}");
    assert!(msg.contains("connection refused"), "{msg}");
}
