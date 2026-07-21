use crate::fetch::Digest;
use crate::pin::Language;

/// The CPU architecture a toolchain is being provisioned for. Vendors spell it
/// differently, hence the per-vendor accessors rather than one string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    /// The architecture this binary is running on, which is the one the
    /// container needs — the entrypoint runs inside the image it provisions for.
    #[must_use]
    pub fn host() -> Self {
        if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X64
        }
    }

    /// This architecture's segment in the shared toolchain cache path.
    ///
    /// The cache is ONE volume shared by every container on the host, and a
    /// toolchain is a native binary, so the arch has to be part of the key.
    /// Without it a mixed-arch host — Docker Desktop on Apple Silicon running an
    /// amd64 image, a multi-arch runner — hands the second container the first
    /// one's toolchain: `is_provisioned` is only "the destination exists", and
    /// the destination was arch-blind. Measured before the fix: an amd64
    /// container reported `go version go1.26.5 linux/arm64`, silently building
    /// for the wrong target rather than failing.
    ///
    /// Docker's spelling (`amd64`/`arm64`), because it is what the user sees in
    /// `kenn docker-cache` and in image platform strings.
    #[must_use]
    pub fn cache_key(self) -> &'static str {
        match self {
            Arch::X64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }

    /// .NET RID fragment — glibc, matching the noble base every image now uses.
    ///
    /// This was briefly musl, when the C# image was alpine. Unifying on one
    /// glibc distro removed that: the vendor's DEFAULT artifact is glibc, so
    /// the supported path is now also the one we take.
    pub(super) fn dotnet_rid(self) -> &'static str {
        match self {
            Arch::X64 => "linux-x64",
            Arch::Arm64 => "linux-arm64",
        }
    }

    /// Go's `arch` field (`amd64`, `arm64`).
    pub(super) fn go_arch(self) -> &'static str {
        match self {
            Arch::X64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }

    /// Node's platform fragment in `index.json`'s `files` and its filenames.
    pub(super) fn node_platform(self) -> &'static str {
        match self {
            Arch::X64 => "linux-x64",
            Arch::Arm64 => "linux-arm64",
        }
    }

    /// python-build-standalone's `arch.family`.
    pub(super) fn python_arch(self) -> &'static str {
        match self {
            Arch::X64 => "x86_64",
            Arch::Arm64 => "aarch64",
        }
    }

    /// Rust target triple — GNU, matching the glibc base.
    ///
    /// The toolchain itself is published for musl too, and alpine would have
    /// been viable on that alone. What forces glibc is the INDEXER:
    /// rust-analyzer's own releases ship `*-unknown-linux-gnu` only, and the
    /// alpine package that would replace it depends on `rust-src`, dragging the
    /// whole toolchain back into the image (measured: 938 MB).
    ///
    /// So the triple follows the base, and the base follows the indexer.
    pub(super) fn rust_triple(self) -> &'static str {
        match self {
            Arch::X64 => "x86_64-unknown-linux-gnu",
            Arch::Arm64 => "aarch64-unknown-linux-gnu",
        }
    }
}

/// How a resolved toolchain is installed into its staging directory.
#[derive(Debug, Clone)]
pub enum Install {
    /// One verified tarball, unpacked in place. The common case.
    Tarball {
        url: String,
        digest_hex: String,
        digest_is_sha512: bool,
        strip_components: usize,
    },
    /// Drive `rustup`, pointed at the staging directory as its `RUSTUP_HOME`.
    ///
    /// Rust is the one language whose toolchain is not a tarball but FOUR
    /// component bundles (rustc, cargo, rust-std, rust-src) that rustup merges
    /// into a sysroot per its manifest. Unpacking them ourselves would mean
    /// reimplementing that merge — which this design explicitly lists as a
    /// non-goal, and which rustup already does while verifying each component
    /// against the same signed manifest we would be reading.
    Rustup { channel: String },
    /// Already placed in the cache by kenn's preflight, on the host.
    ///
    /// Swift alone: swift.org publishes no verifiable tarball, so its toolchain
    /// is copied out of the official `swift:<tag>` image — and only the host can
    /// call docker. The entrypoint therefore expects it present and fails
    /// loudly if it is not, rather than silently indexing without it.
    Preprovisioned,
}

/// A resolved toolchain: how to install it, plus the concrete version that
/// becomes the cache key.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The concrete version the pin resolved to — the cache key, so two pins
    /// that resolve identically share one installation.
    pub version: String,
    pub install: Install,
}

impl Install {
    /// The digest to verify a [`Install::Tarball`] against. `None` for installs
    /// whose vendor tool does its own verification.
    #[must_use]
    pub fn digest(&self) -> Option<Digest<'_>> {
        match self {
            Install::Tarball {
                digest_hex,
                digest_is_sha512,
                ..
            } => Some(if *digest_is_sha512 {
                Digest::Sha512(digest_hex)
            } else {
                Digest::Sha256(digest_hex)
            }),
            Install::Rustup { .. } | Install::Preprovisioned => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("fetching release metadata for {language} from {url}: {message}")]
    Metadata {
        language: &'static str,
        url: String,
        message: String,
    },
    /// The pin names something the vendor does not publish. Names the pin and
    /// where it came from, because "not found" alone sends people hunting in the
    /// wrong place.
    #[error(
        "{language}: no release matches the pinned version {pin:?} (from {pin_source}){detail}"
    )]
    NoMatch {
        language: &'static str,
        pin: String,
        /// NOT named `source`: thiserror reserves that for an error cause.
        pin_source: String,
        detail: String,
    },
    /// A language we cannot verify, and therefore will not provision.
    #[error("{language}: {reason}")]
    Unsupported {
        language: &'static str,
        reason: String,
    },
}

/// What a language resolves to when the workspace declares nothing.
///
/// A workspace with no pin file has no opinion, so any current toolchain will
/// do — but "any" still has to become one concrete version, and hardcoding it
/// would reintroduce exactly the staleness this change removes. So the default
/// is read from the vendor's metadata too: the newest release it publishes.
///
/// `None` means the language has no usable notion of "latest" here and an
/// unpinned workspace is simply not provisioned.
#[must_use]
pub fn default_pin(language: Language) -> Option<&'static str> {
    match language {
        // rustup's own name for "the current release" — the manifest exists
        // under exactly this name, so no version lookup is needed.
        Language::Rust => Some("stable"),
        // Resolved from metadata by the language's own resolver.
        Language::Dotnet | Language::Go | Language::Node | Language::Python => Some(LATEST),
        Language::Swift | Language::TypeScript => None,
    }
}

/// Sentinel pin meaning "whatever the vendor currently publishes as newest".
/// Not a version string any vendor uses, so it cannot collide with a real pin.
pub const LATEST: &str = "*latest*";

/// Resolve `pin` for `language` on `arch`.
///
/// `fetch_text` retrieves a metadata document — injected so the resolvers are
/// testable against recorded fixtures rather than the live internet.
pub fn resolve(
    language: Language,
    pin: &str,
    pin_source: &str,
    roll_forward: Option<&str>,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    match language {
        Language::Go => super::go::resolve_go(pin, pin_source, arch, fetch_text),
        Language::Rust => super::rust::resolve_rust(pin, pin_source, arch, fetch_text),
        // Swift is the one language with NO verifiable download. swift.org
        // publishes neither a URL nor a checksum for Linux toolchains — a
        // release entry carries only {name, platform, archs, docker}, the
        // .sha256 and SHA256SUMS endpoints 404, and the only integrity artifact
        // is a detached PGP signature. (The `checksum` fields that DO appear in
        // that JSON belong to the static-sdk / wasm-sdk pseudo-platforms, not
        // the toolchain tarball; reading them would verify the wrong file and
        // call it safe.)
        //
        // So the toolchain is copied out of the official `swift:<tag>` image
        // instead, where registry content-addressing verifies every layer. Only
        // the host can call docker, so it arrives already provisioned and the
        // pin is simply the version to look up.
        //
        // `swift-tools-version` is a MINIMUM, not an exact version, so this is
        // approximate in a way the other languages are not — worth revisiting if
        // a workspace ever needs an exact Swift.
        Language::Swift => Ok(super::swift::resolve_swift(pin)),
        Language::Dotnet => {
            super::dotnet::resolve_dotnet(pin, pin_source, roll_forward, arch, fetch_text)
        }
        Language::Node => super::node::resolve_node(pin, pin_source, arch, fetch_text),
        Language::Python => super::python::resolve_python(pin, pin_source, arch, fetch_text),
        // Unreachable in practice: `default_pin` is None and there is no pin
        // file, so `provision` returns NotPinned before reaching a resolver.
        Language::TypeScript => Err(ResolveError::Unsupported {
            language: "typescript",
            reason: "the TypeScript indexer embeds its own runtime; nothing to provision"
                .to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic resolve-error contract: a metadata fetch failure names the
    /// language and the URL it was fetching, whichever language triggered it.
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
}
