use super::common::{Arch, Install, ResolveError, Resolved};

fn rust_manifest_url(channel: &str) -> String {
    // `nightly-2026-07-16` is a dated manifest; everything else (a version, or a
    // named channel like `stable`) has a manifest of its own name.
    if let Some(date) = channel.strip_prefix("nightly-") {
        format!("https://static.rust-lang.org/dist/{date}/channel-rust-nightly.toml")
    } else {
        format!("https://static.rust-lang.org/dist/channel-rust-{channel}.toml")
    }
}

pub(super) fn resolve_rust(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let url = rust_manifest_url(pin);
    let body = fetch_text(&url).map_err(|message| ResolveError::Metadata {
        language: "rust",
        url: url.clone(),
        message,
    })?;
    let doc: toml::Value = toml::from_str(&body).map_err(|e| ResolveError::Metadata {
        language: "rust",
        url: url.clone(),
        message: e.to_string(),
    })?;

    // What rust-analyzer needs at index time is the TOOLCHAIN: `cargo` to run
    // `cargo metadata`, and `rust-src` to resolve std. rust-analyzer itself is
    // the INDEXER and ships in the image, so it is not what we provision.
    //
    // Availability is checked on cargo, the component whose absence would be
    // least obvious — a missing rustc fails immediately, a missing cargo fails
    // only once a target is opened.
    let available = doc
        .get("pkg")
        .and_then(|p| p.get("cargo"))
        .and_then(|c| c.get("target"))
        .and_then(|t| t.get(arch.rust_triple()))
        .and_then(|e| e.get("available"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    if !available {
        return Err(ResolveError::NoMatch {
            language: "rust",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(
                " — the toolchain is not available for {} in that channel",
                arch.rust_triple()
            ),
        });
    }

    // The manifest's own `version` for a component is not the release version
    // (cargo says 0.98.0 inside 1.97.1), so take the release's `date`-stamped
    // version from the top level instead.
    let version = doc
        .get("pkg")
        .and_then(|p| p.get("rust"))
        .and_then(|r| r.get("version"))
        .and_then(toml::Value::as_str)
        .and_then(|v| v.split_whitespace().next())
        .unwrap_or(pin)
        .to_string();

    Ok(Resolved {
        version,
        install: Install::Rustup {
            channel: pin.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::Language;
    use crate::resolve::resolve;
    use crate::resolve::testutil::fetcher;

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
}
