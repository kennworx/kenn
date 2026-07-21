//! `resolve` tests for go. Shared `tarball`/`fetcher` helpers come from
//! the parent test module.

use super::super::*;
use super::{fetcher, tarball};
use crate::pin::Language;

const GO_JSON: &str = r#"[
      {"version":"go1.26.5","stable":true,"files":[
        {"filename":"go1.26.5.linux-arm64.tar.gz","os":"linux","arch":"arm64",
         "version":"go1.26.5","sha256":"fe4789e9","size":1,"kind":"archive"}]},
      {"version":"go1.24.5","stable":true,"files":[
        {"filename":"go1.24.5.src.tar.gz","os":"","arch":"","version":"go1.24.5",
         "sha256":"src000","size":1,"kind":"source"},
        {"filename":"go1.24.5.linux-amd64.tar.gz","os":"linux","arch":"amd64",
         "version":"go1.24.5","sha256":"amd64hash","size":1,"kind":"archive"},
        {"filename":"go1.24.5.linux-arm64.tar.gz","os":"linux","arch":"arm64",
         "version":"go1.24.5","sha256":"arm64hash","size":1,"kind":"archive"}]}
    ]"#;

#[test]
fn go_selects_the_archive_for_the_pinned_version_and_arch() {
    let got = resolve(
        Language::Go,
        "1.24.5",
        "go.mod",
        None,
        Arch::Arm64,
        &fetcher(GO_JSON),
    )
    .expect("resolve");
    assert_eq!(got.version, "1.24.5");
    assert_eq!(
        tarball(&got).0,
        "https://go.dev/dl/go1.24.5.linux-arm64.tar.gz"
    );
    assert_eq!(tarball(&got).1, "arm64hash");
    assert!(!tarball(&got).2);
    assert_eq!(tarball(&got).3, 1);

    let amd = resolve(
        Language::Go,
        "1.24.5",
        "go.mod",
        None,
        Arch::X64,
        &fetcher(GO_JSON),
    )
    .expect("resolve");
    assert_eq!(tarball(&amd).1, "amd64hash");
}

/// `kind: source` is not a toolchain. Selecting it would download something
/// that unpacks fine and then fails much later as a missing `bin/go`.
#[test]
fn go_ignores_the_source_archive() {
    let got = resolve(
        Language::Go,
        "1.24.5",
        "go.mod",
        None,
        Arch::X64,
        &fetcher(GO_JSON),
    )
    .expect("resolve");
    assert!(
        tarball(&got).0.ends_with("linux-amd64.tar.gz"),
        "{}",
        tarball(&got).0
    );
}

/// An unmatched pin must name the pin AND where it came from — "not found"
/// alone sends people hunting in the wrong file.
#[test]
fn go_reports_the_pin_and_its_source_when_nothing_matches() {
    let err = resolve(
        Language::Go,
        "1.99.0",
        "/ws/go.mod",
        None,
        Arch::X64,
        &fetcher(GO_JSON),
    )
    .expect_err("no such version");
    let msg = err.to_string();
    assert!(msg.contains("1.99.0"), "{msg}");
    assert!(msg.contains("/ws/go.mod"), "{msg}");
}

#[test]
fn go_accepts_a_pin_written_with_the_go_prefix() {
    let got = resolve(
        Language::Go,
        "go1.24.5",
        "go.mod",
        None,
        Arch::X64,
        &fetcher(GO_JSON),
    )
    .expect("resolve");
    assert_eq!(got.version, "1.24.5", "the prefix is normalized away");
}
