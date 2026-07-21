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

    mod dotnet;
    mod go;
    mod misc;
    mod node;
    mod python;
    mod rust;
}
