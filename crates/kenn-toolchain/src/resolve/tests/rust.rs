//! `resolve` tests for rust. Shared `tarball`/`fetcher` helpers come from
//! the parent test module.

use super::super::*;
use super::fetcher;
use crate::pin::Language;

const RUST_TOML: &str = r#"
manifest-version = "2"
date = "2026-07-16"
[renames.rust-analyzer]
to = "rust-analyzer-preview"
[pkg.rust]
version = "1.97.1 (abc 2026-07-16)"
[pkg.cargo.target.aarch64-unknown-linux-gnu]
available = true
url = "https://static.rust-lang.org/dist/cargo-1.97.1-aarch64-unknown-linux-gnu.tar.gz"
hash = "c0ffee"
[pkg.cargo.target.x86_64-unknown-linux-gnu]
available = false
[pkg.rust-analyzer-preview.target.aarch64-unknown-linux-gnu]
available = true
url = "https://static.rust-lang.org/dist/2026-07-16/rust-analyzer-1.97.1-aarch64-unknown-linux-gnu.tar.gz"
hash = "9d3921c3"
xz_url = "https://static.rust-lang.org/dist/2026-07-16/rust-analyzer-1.97.1-aarch64-unknown-linux-gnu.tar.xz"
xz_hash = "143c111d"
[pkg.rust-analyzer-preview.target.x86_64-unknown-linux-gnu]
available = false
"#;

#[test]
fn rust_reads_the_absolute_url_and_hash_from_the_manifest() {
    let got = resolve(
        Language::Rust,
        "1.97.1",
        "rust-toolchain.toml",
        None,
        Arch::Arm64,
        &fetcher(RUST_TOML),
    )
    .expect("resolve");
    // Rust installs via rustup, not a tarball of ours: its toolchain is four
    // component bundles that rustup merges into a sysroot per the manifest.
    match &got.install {
        Install::Rustup { channel } => assert_eq!(channel, "1.97.1"),
        other => panic!("rust must install via rustup, got {other:?}"),
    }
    // The RELEASE version, not a component's own `version` string — cargo
    // reports 0.98.0 inside the 1.97.1 release, so keying the cache on a
    // component version would collide across unrelated releases.
    assert_eq!(got.version, "1.97.1");
    // And nothing of ours to verify, because rustup verifies each component
    // against the same signed manifest.
    assert!(got.install.digest().is_none());
}

/// `available = false` means the vendor did not build it. Taking the `url`
/// anyway yields a 404 at download time instead of a clear message here.
#[test]
fn rust_refuses_an_unavailable_target() {
    let err = resolve(
        Language::Rust,
        "1.97.1",
        "rust-toolchain.toml",
        None,
        Arch::X64,
        &fetcher(RUST_TOML),
    )
    .expect_err("unavailable target");
    assert!(
        err.to_string().contains("x86_64-unknown-linux-gnu"),
        "{err}"
    );
}

#[test]
fn rust_manifest_url_handles_channels_versions_and_dated_nightlies() {
    assert_eq!(
        rust_manifest_url("stable"),
        "https://static.rust-lang.org/dist/channel-rust-stable.toml"
    );
    assert_eq!(
        rust_manifest_url("1.97.1"),
        "https://static.rust-lang.org/dist/channel-rust-1.97.1.toml"
    );
    assert_eq!(
        rust_manifest_url("nightly-2026-07-16"),
        "https://static.rust-lang.org/dist/2026-07-16/channel-rust-nightly.toml"
    );
}
