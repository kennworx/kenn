use super::common::{Install, Resolved};

/// Swift is the one language with NO verifiable download. swift.org publishes
/// neither a URL nor a checksum for Linux toolchains — a release entry carries
/// only `{name, platform, archs, docker}`, the `.sha256` and `SHA256SUMS`
/// endpoints 404, and the only integrity artifact is a detached PGP signature.
/// (The `checksum` fields that DO appear in that JSON belong to the static-sdk /
/// wasm-sdk pseudo-platforms, not the toolchain tarball; reading them would
/// verify the wrong file and call it safe.)
///
/// So the toolchain is copied out of the official `swift:<tag>` image instead,
/// where registry content-addressing verifies every layer. Only the host can
/// call docker, so it arrives already provisioned and the pin is simply the
/// version to look up.
///
/// `swift-tools-version` is a MINIMUM, not an exact version, so this is
/// approximate in a way the other languages are not — worth revisiting if a
/// workspace ever needs an exact Swift.
pub(super) fn resolve_swift(pin: &str) -> Resolved {
    Resolved {
        version: pin.to_string(),
        install: Install::Preprovisioned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::Language;
    use crate::resolve::testutil::fetcher;
    use crate::resolve::{resolve, Arch};

    /// Swift must be refused, not provisioned unverified. If someone later makes
    /// this "work" by reading the static-sdk `checksum`, they will be verifying
    /// a different file and calling it safe.
    #[test]
    fn swift_is_refused_because_it_cannot_be_verified() {
        let got = resolve(
            Language::Swift,
            "6.0",
            "Package.swift",
            None,
            Arch::Arm64,
            &fetcher("{}"),
        )
        .expect("swift resolves to a host-provisioned toolchain");
        assert!(
            matches!(got.install, Install::Preprovisioned),
            "{:?}",
            got.install
        );
        assert_eq!(got.version, "6.0");
        assert!(got.install.digest().is_none(), "nothing of ours to verify");
    }
}
