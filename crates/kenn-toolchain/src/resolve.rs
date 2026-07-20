//! Turning a [`Pin`](crate::pin::Pin) into a concrete downloadable artifact,
//! using each vendor's published release metadata.
//!
//! # Why metadata rather than a URL template
//!
//! Artifact URLs are not derivable. .NET's live under
//! `/dotnet/Sdk/<version>/dotnet-sdk-<version>-<rid>.tar.gz` but the *release*
//! that contains a given SDK is not its own version (SDK 9.0.308 ships in
//! release 9.0.11), and Rust's filenames use the release version while the
//! component's own `version` field says something else entirely (`cargo` reports
//! 0.98.0 inside the 1.97.1 release). Guessing works until it doesn't, and then
//! it fails as a 404 months later rather than at review time.
//!
//! The metadata is also where the checksum lives, so reading it is not extra
//! work — it is the only way to satisfy verification at all.
//!
//! # What each vendor actually publishes
//!
//! | language | URL | digest |
//! |---|---|---|
//! | rust | absolute, in the channel manifest | SHA-256 |
//! | dotnet | absolute, in `releases.json` | **SHA-512** |
//! | go | filename only, fixed base | SHA-256 |
//! | node | filename only, from `SHASUMS256.txt` | SHA-256 |
//! | python | absolute, in uv's index | SHA-256 |
//! | swift | **neither** — see [`resolve`] | — |

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
    fn dotnet_rid(self) -> &'static str {
        match self {
            Arch::X64 => "linux-x64",
            Arch::Arm64 => "linux-arm64",
        }
    }

    /// Go's `arch` field (`amd64`, `arm64`).
    fn go_arch(self) -> &'static str {
        match self {
            Arch::X64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }

    /// Node's platform fragment in `index.json`'s `files` and its filenames.
    fn node_platform(self) -> &'static str {
        match self {
            Arch::X64 => "linux-x64",
            Arch::Arm64 => "linux-arm64",
        }
    }

    /// python-build-standalone's `arch.family`.
    fn python_arch(self) -> &'static str {
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
    fn rust_triple(self) -> &'static str {
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
        Language::Go => resolve_go(pin, pin_source, arch, fetch_text),
        Language::Rust => resolve_rust(pin, pin_source, arch, fetch_text),
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
        Language::Swift => Ok(Resolved {
            version: pin.to_string(),
            install: Install::Preprovisioned,
        }),
        Language::Dotnet => resolve_dotnet(pin, pin_source, roll_forward, arch, fetch_text),
        Language::Node => resolve_node(pin, pin_source, arch, fetch_text),
        Language::Python => resolve_python(pin, pin_source, arch, fetch_text),
        // Unreachable in practice: `default_pin` is None and there is no pin
        // file, so `provision` returns NotPinned before reaching a resolver.
        Language::TypeScript => Err(ResolveError::Unsupported {
            language: "typescript",
            reason: "the TypeScript indexer embeds its own runtime; nothing to provision"
                .to_string(),
        }),
    }
}

const GO_METADATA: &str = "https://go.dev/dl/?mode=json&include=all";
/// Go publishes only a filename, so the base is ours. It is a fixed constant
/// rather than a per-artifact guess, and the filename itself is authoritative.
const GO_DOWNLOAD_BASE: &str = "https://go.dev/dl/";

fn resolve_go(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let body = fetch_text(GO_METADATA).map_err(|message| ResolveError::Metadata {
        language: "go",
        url: GO_METADATA.to_string(),
        message,
    })?;
    let doc: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ResolveError::Metadata {
            language: "go",
            url: GO_METADATA.to_string(),
            message: e.to_string(),
        })?;

    // go.dev spells versions with a `go` prefix; our pins are normalized without.
    let want = format!("go{}", pin.trim_start_matches("go"));
    let release = doc.as_array().into_iter().flatten().find(|r| {
        if pin == LATEST {
            // The list is newest-first, so the first stable entry is the latest.
            r.get("stable").and_then(serde_json::Value::as_bool) == Some(true)
        } else {
            r.get("version").and_then(serde_json::Value::as_str) == Some(want.as_str())
        }
    });

    let file = release.and_then(|r| {
        r.get("files")?.as_array()?.iter().find(|f| {
            f.get("kind").and_then(serde_json::Value::as_str) == Some("archive")
                && f.get("os").and_then(serde_json::Value::as_str) == Some("linux")
                && f.get("arch").and_then(serde_json::Value::as_str) == Some(arch.go_arch())
        })
    });

    let (Some(file), Some(filename), Some(sha256)) = (
        file,
        file.and_then(|f| f.get("filename"))
            .and_then(serde_json::Value::as_str),
        file.and_then(|f| f.get("sha256"))
            .and_then(serde_json::Value::as_str),
    ) else {
        return Err(ResolveError::NoMatch {
            language: "go",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: if release.is_some() {
                format!(
                    " — release exists but publishes no linux/{} archive",
                    arch.go_arch()
                )
            } else {
                String::new()
            },
        });
    };
    let _ = file;

    Ok(Resolved {
        // Under LATEST the pin is a sentinel, so the CONCRETE version has to
        // come from the release we picked — the cache key must never be
        // "*latest*", or every new release would silently reuse the old install.
        version: release
            .and_then(|r| r.get("version"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(pin)
            .trim_start_matches("go")
            .to_string(),
        install: Install::Tarball {
            url: format!("{GO_DOWNLOAD_BASE}{filename}"),
            digest_hex: sha256.to_string(),
            digest_is_sha512: false,
            // The tarball wraps everything in a `go/` directory.
            strip_components: 1,
        },
    })
}

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

fn resolve_dotnet(
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

const NODE_INDEX: &str = "https://nodejs.org/dist/index.json";
const NODE_DIST: &str = "https://nodejs.org/dist/";

/// Node ships glibc builds only — musl exists solely on
/// unofficial-builds.nodejs.org, whose retention and architecture coverage we do
/// not control. That is why the python image is glibc-based.
fn resolve_node(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let meta = |url: &str, message: String| ResolveError::Metadata {
        language: "node",
        url: url.to_string(),
        message,
    };
    let body = fetch_text(NODE_INDEX).map_err(|m| meta(NODE_INDEX, m))?;
    let index: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| meta(NODE_INDEX, e.to_string()))?;

    // Entries are newest-first. An unpinned workspace takes the newest LTS
    // rather than the newest release: `lts` is a codename string on LTS lines
    // and `false` otherwise, and a current-but-not-LTS Node is a worse default
    // for running a third-party indexer.
    let entry = index.as_array().into_iter().flatten().find(|r| {
        let version = r.get("version").and_then(serde_json::Value::as_str);
        if pin == LATEST {
            r.get("lts").is_some_and(serde_json::Value::is_string)
        } else {
            version == Some(format!("v{}", pin.trim_start_matches('v')).as_str())
        }
    });
    let (Some(entry), Some(version)) = (
        entry,
        entry
            .and_then(|e| e.get("version"))
            .and_then(serde_json::Value::as_str),
    ) else {
        return Err(ResolveError::NoMatch {
            language: "node",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: String::new(),
        });
    };
    // `files` lists platforms, not filenames — presence check only.
    let has_platform = entry
        .get("files")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|f| f.iter().any(|v| v.as_str() == Some(arch.node_platform())));
    if !has_platform {
        return Err(ResolveError::NoMatch {
            language: "node",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(" — {version} publishes no {} build", arch.node_platform()),
        });
    }

    // The checksum lives in a separate per-version manifest, and its left column
    // is the AUTHORITATIVE filename — so the URL is a fixed base joined to a
    // published name rather than a name we invented.
    let sums_url = format!("{NODE_DIST}{version}/SHASUMS256.txt");
    let sums = fetch_text(&sums_url).map_err(|m| meta(&sums_url, m))?;
    let want = format!("node-{version}-{}.tar.gz", arch.node_platform());
    let sha = sums.lines().find_map(|line| {
        let (sha, name) = line.split_once("  ")?;
        (name.trim() == want).then(|| sha.to_string())
    });
    let Some(sha) = sha else {
        return Err(ResolveError::NoMatch {
            language: "node",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(" — {want} is absent from SHASUMS256.txt"),
        });
    };

    Ok(Resolved {
        version: version.trim_start_matches('v').to_string(),
        install: Install::Tarball {
            url: format!("{NODE_DIST}{version}/{want}"),
            digest_hex: sha,
            digest_is_sha512: false,
            // Wrapped in `node-v<ver>-<platform>/`.
            strip_components: 1,
        },
    })
}

/// uv's curated index over python-build-standalone. PBS itself has no notion of
/// "the newest 3.12", so this is the only machine-readable place a partial pin
/// like `3.12` becomes a concrete asset.
const PYTHON_INDEX: &str =
    "https://raw.githubusercontent.com/astral-sh/uv/main/crates/uv-python/download-metadata.json";

/// Whether a python-build-standalone key names a prerelease.
///
/// Keys look like `cpython-3.12.13-linux-x86_64-gnu` or
/// `cpython-3.15.0b3-linux-x86_64-gnu`. `CPython` spells prereleases with a
/// trailing `aN`/`bN`/`rcN` on the version segment, so the test is whether that
/// segment ends in something other than a digit.
fn key_is_prerelease(key: &str) -> bool {
    key.split('-')
        .nth(1)
        .and_then(|v| v.split('+').next())
        .is_some_and(|v| {
            !v.ends_with(|c: char| c.is_ascii_digit())
                || v.contains('a')
                || v.contains('b')
                || v.contains("rc")
        })
}

fn resolve_python(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let body = fetch_text(PYTHON_INDEX).map_err(|message| ResolveError::Metadata {
        language: "python",
        url: PYTHON_INDEX.to_string(),
        message,
    })?;
    let index: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ResolveError::Metadata {
            language: "python",
            url: PYTHON_INDEX.to_string(),
            message: e.to_string(),
        })?;

    // A pin may be `3.12` or `3.12.13`; both select by prefix, and the newest
    // matching patch wins.
    let wanted: Vec<u64> = pin.split('.').filter_map(|p| p.parse().ok()).collect();
    // A pin naming a prerelease opts into them; nothing else does. Tested on the
    // VERSION shape, not by substring: the LATEST sentinel is "*latest*", which
    // contains an 'a' and so opted every unpinned workspace into prereleases —
    // the exact bug this filter exists to stop.
    let want_prerelease = pin != LATEST && key_is_prerelease(&format!("x-{pin}"));
    let mut best: Option<(u64, String, String)> = None;
    for (key, entry) in index.as_object().into_iter().flatten() {
        // The KEY is the only place a prerelease is visible. uv's index carries
        // `cpython-3.15.0b3-…` alongside finals, and its numeric `major`/`minor`
        // /`patch` fields are 3/15/0 for BOTH — so comparing versions cannot
        // tell them apart, and "newest 3.15.0" silently selected an alpha.
        //
        // Measured: an unpinned real repo provisioned Python 3.15.0a1 and
        // indexed 1202 defs without complaint. A prerelease interpreter is not
        // wrong enough to fail, which is exactly why it has to be excluded here
        // rather than caught downstream.
        //
        // `+freethreaded` / `+debug` are build VARIANTS, not prereleases, and
        // are already excluded by the `.tar.gz` filter below.
        if !want_prerelease && key_is_prerelease(key) {
            continue;
        }
        let get = |k: &str| entry.get(k).and_then(serde_json::Value::as_u64);
        let s = |k: &str| entry.get(k).and_then(serde_json::Value::as_str);
        let (Some(major), Some(minor), Some(patch)) = (get("major"), get("minor"), get("patch"))
        else {
            continue;
        };
        let matches_pin = pin == LATEST
            || (wanted.first() == Some(&major)
                && wanted.get(1).is_none_or(|m| *m == minor)
                && wanted.get(2).is_none_or(|p| *p == patch));
        let right_platform = s("name") == Some("cpython")
            && s("os") == Some("linux")
            // glibc: the python image is noble-based, and a musl build would
            // resolve cleanly and then fail to exec.
            && s("libc") == Some("gnu")
            && entry
                .get("arch")
                .and_then(|a| a.get("family"))
                .and_then(serde_json::Value::as_str)
                == Some(arch.python_arch());
        if !matches_pin || !right_platform {
            continue;
        }
        let (Some(url), Some(sha)) = (s("url"), s("sha256")) else {
            continue;
        };
        // The index also carries `debug-full` and `pgo+lto-full` variants, which
        // are `.tar.zst`. We unpack gzip only, and selecting on version alone
        // picked one — the failure surfaced as "invalid gzip header" after a
        // successful resolve, naming neither the variant nor why it was chosen.
        // `install_only*` builds are the runnable ones and the only `.tar.gz`.
        if !url.ends_with(".tar.gz") {
            continue;
        }
        let key = major * 1_000_000 + minor * 1_000 + patch;
        if best.as_ref().is_none_or(|(k, _, _)| key > *k) {
            best = Some((key, url.to_string(), sha.to_string()));
        }
    }

    let Some((key, url, sha)) = best else {
        return Err(ResolveError::NoMatch {
            language: "python",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(" — for linux/{} (gnu)", arch.python_arch()),
        });
    };

    Ok(Resolved {
        version: format!(
            "{}.{}.{}",
            key / 1_000_000,
            (key / 1_000) % 1_000,
            key % 1_000
        ),
        install: Install::Tarball {
            url,
            digest_hex: sha,
            digest_is_sha512: false,
            // `install_only` tarballs wrap everything in `python/`.
            strip_components: 1,
        },
    })
}

fn rust_manifest_url(channel: &str) -> String {
    // `nightly-2026-07-16` is a dated manifest; everything else (a version, or a
    // named channel like `stable`) has a manifest of its own name.
    if let Some(date) = channel.strip_prefix("nightly-") {
        format!("https://static.rust-lang.org/dist/{date}/channel-rust-nightly.toml")
    } else {
        format!("https://static.rust-lang.org/dist/channel-rust-{channel}.toml")
    }
}

fn resolve_rust(
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

    /// Unwrap a Tarball install, failing loudly on any other variant — a test
    /// asserting on a URL must not silently pass because the shape changed.
    fn tarball(r: &Resolved) -> (String, String, bool, usize) {
        match &r.install {
            Install::Tarball {
                url,
                digest_hex,
                digest_is_sha512,
                strip_components,
            } => (
                url.clone(),
                digest_hex.clone(),
                *digest_is_sha512,
                *strip_components,
            ),
            other => panic!("expected a Tarball install, got {other:?}"),
        }
    }

    fn fetcher(body: &'static str) -> impl Fn(&str) -> Result<String, String> {
        move |_: &str| Ok(body.to_string())
    }

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

    const NODE_INDEX_JSON: &str = r#"[
      {"version":"v24.4.0","files":["linux-x64","linux-arm64"],"lts":false},
      {"version":"v22.20.0","files":["linux-x64","linux-arm64"],"lts":"Jod"},
      {"version":"v20.19.0","files":["linux-x64"],"lts":"Iron"}
    ]"#;
    const NODE_SHASUMS: &str = concat!(
        "aaaa  node-v22.20.0-linux-x64.tar.xz\n",
        "1111  node-v22.20.0-linux-x64.tar.gz\n",
        "2222  node-v22.20.0-linux-arm64.tar.gz\n"
    );

    #[expect(
        clippy::unnecessary_wraps,
        reason = "must match the fetch_text signature it is passed as"
    )]
    fn node_fetcher(url: &str) -> Result<String, String> {
        Ok(if url.contains("SHASUMS") {
            NODE_SHASUMS
        } else {
            NODE_INDEX_JSON
        }
        .to_string())
    }

    #[test]
    fn node_takes_the_gz_checksum_for_the_right_platform() {
        let got = resolve(
            Language::Node,
            "22.20.0",
            "<none>",
            None,
            Arch::Arm64,
            &node_fetcher,
        )
        .expect("resolve");
        assert_eq!(got.version, "22.20.0");
        assert_eq!(tarball(&got).1, "2222");
        assert!(tarball(&got)
            .0
            .ends_with("node-v22.20.0-linux-arm64.tar.gz"));
        // `.tar.xz` shares the platform and sorts first in SHASUMS256.txt;
        // matching on the platform alone would take its checksum.
        assert_ne!(tarball(&got).1, "aaaa");
    }

    /// Unpinned Node takes the newest LTS, not the newest release: a
    /// current-but-not-LTS Node is a worse host for a third-party indexer.
    #[test]
    fn node_defaults_to_the_newest_lts_not_the_newest_release() {
        let got = resolve(
            Language::Node,
            LATEST,
            "<none>",
            None,
            Arch::X64,
            &node_fetcher,
        )
        .expect("resolve");
        assert_eq!(got.version, "22.20.0", "24.4.0 is newer but not LTS");
    }

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
}
