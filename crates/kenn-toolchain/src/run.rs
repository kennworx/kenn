//! The entrypoint's actual job: pin → resolve → provision → report.
//!
//! # Everything here writes to stderr, never stdout
//!
//! Three of the six indexers stream JSONL frames on stdout, and kenn parses that
//! stream. A single stray byte from the entrypoint would corrupt a frame, and
//! the failure would look like an indexer bug rather than a provisioning one.
//! kenn already captures each driver's stderr, so that is the channel.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cache::{CacheError, ToolchainCache};
use crate::fetch::{self, Artifact, FetchError};
use crate::pin::{self, Language, PinError};
use crate::resolve::{self, Arch, ResolveError};

/// Env var naming the mounted toolchain cache. Set by kenn's docker runtime; its
/// absence means this image was run outside that runtime, so there is nowhere to
/// provision into and nothing to do.
pub const CACHE_ROOT_ENV: &str = "KENN_TOOLCHAIN_ROOT";

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Pin(#[from] PinError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error("installing the .NET SDK into the toolchain root: {0}")]
    Io(#[from] std::io::Error),
}

/// What provisioning did, for the caller to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No cache is mounted — this image is running outside kenn's docker runtime.
    NoCache,
    /// The workspace pins nothing and the language has no "latest" to fall back
    /// on, so there is nothing to provision.
    NotPinned,
    /// The toolchain was already present.
    AlreadyPresent { version: String, path: PathBuf },
    /// The toolchain was downloaded and installed.
    Provisioned { version: String, path: PathBuf },
}

impl Outcome {
    /// The provisioned toolchain's root, for the caller to export.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Outcome::AlreadyPresent { path, .. } | Outcome::Provisioned { path, .. } => Some(path),
            Outcome::NoCache | Outcome::NotPinned => None,
        }
    }

    /// The resolved toolchain version, when one was provisioned or already
    /// present. `None` when nothing was provisioned (no cache, or the workspace
    /// pins nothing) — there is then no version to attribute the index to.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        match self {
            Outcome::AlreadyPresent { version, .. } | Outcome::Provisioned { version, .. } => {
                Some(version)
            }
            Outcome::NoCache | Outcome::NotPinned => None,
        }
    }
}

/// Resolve and provision `language`'s toolchain for the workspace at `workspace`.
///
/// `progress` receives human-readable status lines; the caller points it at
/// stderr. It is written to BEFORE the download starts, not after: a first
/// provision moves hundreds of megabytes, and a silent producer during that
/// window is indistinguishable from a hung one.
pub fn provision(
    language: Language,
    workspace: &Path,
    arch: Arch,
    progress: &mut dyn Write,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
    install: &dyn Fn(Artifact<'_>, &Path) -> Result<(), FetchError>,
) -> Result<Outcome, RunError> {
    let Some(cache_root) = std::env::var_os(CACHE_ROOT_ENV) else {
        return Ok(Outcome::NoCache);
    };
    let cache = ToolchainCache::new(PathBuf::from(cache_root), arch.cache_key());

    let found = pin::find_pin(language, workspace)?;
    let (pin_text, pin_source, roll_forward) = match &found {
        Some(p) => (
            p.version.clone(),
            p.source.display().to_string(),
            p.roll_forward.clone(),
        ),
        None => match resolve::default_pin(language) {
            Some(default) => (default.to_string(), "<no pin file>".to_string(), None),
            None => return Ok(Outcome::NotPinned),
        },
    };

    let resolved = resolve::resolve(
        language,
        &pin_text,
        &pin_source,
        roll_forward.as_deref(),
        arch,
        fetch_text,
    )?;

    if cache.is_provisioned(language.key(), &resolved.version) {
        return Ok(Outcome::AlreadyPresent {
            path: cache.path(language.key(), &resolved.version),
            version: resolved.version,
        });
    }

    // Announced BEFORE the download. See the module docs.
    drop(writeln!(
        progress,
        "kenn-toolchain: provisioning {} {} (pinned in {pin_source})",
        language.key(),
        resolved.version
    ));
    drop(progress.flush());

    let path = cache.provision(
        language.key(),
        &resolved.version,
        |staging| match &resolved.install {
            resolve::Install::Tarball {
                url,
                digest_hex,
                digest_is_sha512,
                strip_components,
            } => install(
                Artifact {
                    url,
                    digest: if *digest_is_sha512 {
                        crate::fetch::Digest::Sha512(digest_hex)
                    } else {
                        crate::fetch::Digest::Sha256(digest_hex)
                    },
                    strip_components: *strip_components,
                },
                staging,
            )
            .map_err(|e| e.to_string()),
            resolve::Install::Rustup { channel } => rustup_install(channel, staging),
            // Reached only when the cache does NOT already hold it — the check
            // above returns AlreadyPresent otherwise. So this is exactly the
            // "host preflight did not run, or ran for a different version" case,
            // and indexing without the toolchain is the failure being avoided.
            resolve::Install::Preprovisioned => Err(format!(
                "no Swift toolchain in the cache. It is provisioned on the host \
                 from the official swift image, not downloaded here — run kenn's \
                 preflight, or index with runtime = \"local\". Staging: {}",
                staging.display()
            )),
        },
    )?;

    drop(writeln!(
        progress,
        "kenn-toolchain: provisioned {} {}",
        language.key(),
        resolved.version
    ));
    Ok(Outcome::Provisioned {
        version: resolved.version,
        path,
    })
}

/// Install one specific .NET SDK version INTO an existing `dotnet_root`, beside
/// whatever SDK is already there.
///
/// This is the nested-`global.json` case: kenn provisions the SDK the ROOT pin
/// names, but a subdirectory's `global.json` can pin a different one, and the
/// Roslyn `BuildHost` resolves each project's pin against the SDKs in a SINGLE
/// `DOTNET_ROOT`. .NET is built for that — one root holds `sdk/<a>`, `sdk/<b>`,
/// … side by side — so kenn-dotnet asks for the missing version and it lands
/// alongside, with no re-registration.
///
/// Atomicity, without a whole-tree rename: the tarball is unpacked into a
/// staging dir on the same filesystem, its runtime/host pieces are merged in
/// first (additive — shared versioned subdirs the existing SDK also uses), and
/// `sdk/<version>` is renamed into place LAST. hostfxr never selects an
/// incomplete `sdk/<version>`, so a crash before that final rename leaves
/// nothing selectable and the next run completes it.
pub fn provision_sdk(
    version: &str,
    roll_forward: &str,
    dotnet_root: &Path,
    arch: Arch,
    progress: &mut dyn Write,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
    install: &dyn Fn(Artifact<'_>, &Path) -> Result<(), FetchError>,
) -> Result<String, RunError> {
    let roll = (!roll_forward.is_empty()).then_some(roll_forward);
    let resolved = resolve::resolve(
        Language::Dotnet,
        version,
        "global.json",
        roll,
        arch,
        fetch_text,
    )?;

    let sdk_dir = dotnet_root.join("sdk").join(&resolved.version);
    if sdk_dir.is_dir() {
        // Already installed by an earlier run or another project. Reuse.
        return Ok(resolved.version);
    }

    let artifact = dotnet_tarball_artifact(&resolved.install)?;

    drop(writeln!(
        progress,
        "kenn-toolchain: provisioning .NET SDK {} into {}",
        resolved.version,
        dotnet_root.display()
    ));
    drop(progress.flush());

    let staging = dotnet_root.join(format!(
        ".sdk-staging-{}-{}",
        resolved.version,
        std::process::id()
    ));
    drop(std::fs::remove_dir_all(&staging));
    // fetch_verified writes the archive into `staging`, so it must exist first —
    // the cache's own provision path creates its staging dir the same way.
    std::fs::create_dir_all(&staging)?;
    install(artifact, &staging)?;

    let result = merge_sdk_into_root(&staging, dotnet_root, &resolved.version);
    drop(std::fs::remove_dir_all(&staging));
    result?;

    drop(writeln!(
        progress,
        "kenn-toolchain: provisioned .NET SDK {}",
        resolved.version
    ));
    Ok(resolved.version)
}

/// The downloadable artifact for a resolved .NET SDK. `resolve_dotnet` only ever
/// yields a `Tarball`, so the other arm is unreachable — kept as a named error
/// rather than an `unwrap` so a future resolver change fails loudly.
fn dotnet_tarball_artifact(install: &resolve::Install) -> Result<Artifact<'_>, RunError> {
    match install {
        resolve::Install::Tarball {
            url,
            digest_hex,
            digest_is_sha512,
            strip_components,
        } => Ok(Artifact {
            url,
            digest: if *digest_is_sha512 {
                fetch::Digest::Sha512(digest_hex)
            } else {
                fetch::Digest::Sha256(digest_hex)
            },
            strip_components: *strip_components,
        }),
        _ => Err(RunError::Resolve(ResolveError::Unsupported {
            language: "dotnet",
            reason: "expected a downloadable SDK tarball".to_string(),
        })),
    }
}

/// Move a staged SDK into `dotnet_root`: every component EXCEPT `sdk/` merged
/// additively first, then `sdk/<version>` renamed in as the atomic completion.
fn merge_sdk_into_root(staging: &Path, dotnet_root: &Path, version: &str) -> Result<(), RunError> {
    for entry in std::fs::read_dir(staging)? {
        let entry = entry?;
        if entry.file_name() == "sdk" {
            continue; // the completion marker; renamed last.
        }
        merge_additive(&entry.path(), &dotnet_root.join(entry.file_name()))?;
    }
    std::fs::create_dir_all(dotnet_root.join("sdk"))?;
    std::fs::rename(
        staging.join("sdk").join(version),
        dotnet_root.join("sdk").join(version),
    )?;
    Ok(())
}

/// Recursively move `src` into `dst` WITHOUT overwriting. A path already in
/// `dst` is a versioned runtime/host dir the existing SDK also carries — the
/// same content — so it is left as is. All renames are within one filesystem,
/// so each is cheap and atomic.
fn merge_additive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        return std::fs::rename(src, dst);
    }
    if src.is_dir() && dst.is_dir() {
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            merge_additive(&entry.path(), &dst.join(entry.file_name()))?;
        }
    }
    // A file that already exists is identical shared content — skip.
    Ok(())
}

/// Install a Rust toolchain by driving `rustup`, with `staging` as its
/// `RUSTUP_HOME` so the whole installation lands inside the staging tree and the
/// atomic rename still applies.
///
/// `--profile minimal` plus `rust-src` is exactly what `rust-analyzer` needs:
/// cargo to run `cargo metadata`, rustc for the sysroot, and rust-src to resolve
/// std. The default profile would add docs, clippy and rustfmt — hundreds of
/// megabytes the indexer never opens.
///
/// rustup verifies each component against the same signed channel manifest we
/// read to resolve the version, which is why this path has no digest of ours.
fn rustup_install(channel: &str, staging: &Path) -> Result<(), String> {
    let status = std::process::Command::new("rustup")
        .args([
            "toolchain",
            "install",
            channel,
            "--profile",
            "minimal",
            "--component",
            "rust-src",
            "--no-self-update",
        ])
        .env("RUSTUP_HOME", staging)
        // rustup writes cargo shims into CARGO_HOME; keep them in the staging
        // tree too rather than leaking into the container's HOME.
        .env("CARGO_HOME", staging.join("cargo"))
        .status()
        .map_err(|e| format!("running rustup: {e} (is rustup in the image?)"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "rustup toolchain install {channel} failed with {status}"
        ))
    }
}

/// Where a provisioned toolchain's executables actually live, given its root.
///
/// Not uniform, and not guessable from the language alone:
/// - .NET unpacks `dotnet` at the root of the SDK tree, with no `bin/`.
/// - **rustup** puts them at `toolchains/<channel>-<triple>/bin`. It does NOT
///   create `CARGO_HOME/bin` shims — those come from `rustup-init`, not from
///   `rustup toolchain install`, so pointing at a shim directory finds nothing.
///   The triple is not known here, so the single installed toolchain is located
///   by looking rather than by reconstructing its name.
/// - everything else uses a plain `bin/`.
#[must_use]
pub fn toolchain_bin(language: Language, root: &Path) -> PathBuf {
    match language {
        Language::Dotnet => root.to_path_buf(),
        Language::Rust => std::fs::read_dir(root.join("toolchains"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("bin"))
            .find(|p| p.is_dir())
            .unwrap_or_else(|| root.join("bin")),
        // Copied out of the official image with `cp --parents`, which preserves
        // the leading `/usr` — so the binaries land at `<root>/usr/bin`, not
        // `<root>/bin`. Getting this wrong yields "swiftc: not found" from a
        // toolchain that is fully present.
        Language::Swift => root.join("usr/bin"),
        Language::Go | Language::Python | Language::Node | Language::TypeScript => root.join("bin"),
    }
}

/// The real fetcher, for production use.
pub fn http_text(url: &str) -> Result<String, String> {
    ureq::get(url)
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// The real installer, for production use.
pub fn http_install(artifact: Artifact<'_>, staging: &Path) -> Result<(), FetchError> {
    fetch::fetch_verified(artifact, staging)
}

#[cfg(test)]
mod merge_tests {
    use super::{merge_sdk_into_root, Path};

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// The completion contract: `sdk/<version>` lands, and the runtime/host
    /// pieces merge in beside the existing SDK's without overwriting them.
    #[test]
    fn merges_runtimes_and_lands_the_sdk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dotnet");
        // An existing install: SDK 10 plus a shared runtime it owns.
        write(&root, "sdk/10.0.302/dotnet.dll", "sdk10");
        write(&root, "shared/Microsoft.NETCore.App/10.0.0/x.dll", "rt10");
        write(&root, "host/fxr/10.0.0/libhostfxr.so", "fxr10");

        // A staged SDK 9 tarball: its own sdk dir, its own runtime, and a host
        // file that already exists (identical shared content).
        let staging = tmp.path().join(".staging");
        write(&staging, "sdk/9.0.316/dotnet.dll", "sdk9");
        write(&staging, "shared/Microsoft.NETCore.App/9.0.0/y.dll", "rt9");
        write(&staging, "host/fxr/10.0.0/libhostfxr.so", "DIFFERENT");

        merge_sdk_into_root(&staging, &root, "9.0.316").unwrap();

        // The new SDK is present…
        assert!(root.join("sdk/9.0.316/dotnet.dll").is_file());
        // …beside the old one, untouched…
        assert_eq!(
            std::fs::read_to_string(root.join("sdk/10.0.302/dotnet.dll")).unwrap(),
            "sdk10"
        );
        // …its runtime merged in…
        assert!(root
            .join("shared/Microsoft.NETCore.App/9.0.0/y.dll")
            .is_file());
        // …the old runtime kept…
        assert!(root
            .join("shared/Microsoft.NETCore.App/10.0.0/x.dll")
            .is_file());
        // …and an already-present shared file is NOT overwritten.
        assert_eq!(
            std::fs::read_to_string(root.join("host/fxr/10.0.0/libhostfxr.so")).unwrap(),
            "fxr10"
        );
    }

    // Minimal .NET release metadata: enough for resolve_dotnet to turn pin
    // "9.0.308" into a linux-arm64 tarball URL, with no network.
    const INDEX: &str = r#"{"releases-index":[
      {"channel-version":"9.0","releases.json":"https://x/9.0/releases.json"}]}"#;
    const CHANNEL_9: &str = r#"{"releases":[
      {"release-version":"9.0.11","sdk":{"version":"9.0.308"},"sdks":[
        {"version":"9.0.308","files":[
          {"name":"dotnet-sdk-linux-arm64.tar.gz","rid":"linux-arm64",
           "url":"https://x/sdk-9.0.308-linux-arm64.tar.gz","hash":"AAA"}]}]}]}"#;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "matches the fetch_text Fn signature provision_sdk requires"
    )]
    fn fake_fetch(url: &str) -> Result<String, String> {
        Ok(if url.contains("releases-index") {
            INDEX
        } else {
            CHANNEL_9
        }
        .to_string())
    }

    /// The whole orchestrator, injected end-to-end: resolve the pin, "download"
    /// (a fake that lays down a staged SDK tree), and merge it into a root that
    /// already holds another SDK. The SDK the pin names lands beside the old one.
    #[test]
    fn provision_sdk_resolves_downloads_and_merges() {
        use super::{provision_sdk, Arch};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dotnet");
        std::fs::create_dir_all(root.join("sdk/10.0.302")).unwrap();

        // Fake installer: write a staged SDK layout instead of fetching.
        let install = |_artifact: super::Artifact<'_>, staging: &Path| {
            std::fs::create_dir_all(staging.join("sdk/9.0.308")).unwrap();
            std::fs::write(staging.join("sdk/9.0.308/dotnet.dll"), "sdk9").unwrap();
            std::fs::create_dir_all(staging.join("shared/App/9.0.0")).unwrap();
            std::fs::write(staging.join("shared/App/9.0.0/x.dll"), "rt").unwrap();
            Ok(())
        };

        let mut out = Vec::new();
        let version = provision_sdk(
            "9.0.308",
            "",
            &root,
            Arch::Arm64,
            &mut out,
            &fake_fetch,
            &install,
        )
        .expect("provision");
        assert_eq!(version, "9.0.308");
        assert!(
            root.join("sdk/9.0.308/dotnet.dll").is_file(),
            "new SDK landed"
        );
        assert!(root.join("sdk/10.0.302").is_dir(), "old SDK kept");
        assert!(
            root.join("shared/App/9.0.0/x.dll").is_file(),
            "runtime merged"
        );

        // A second call reuses (installer must NOT run again).
        let ran_again = std::cell::Cell::new(false);
        let install2 = |_a: super::Artifact<'_>, _s: &Path| {
            ran_again.set(true);
            Ok(())
        };
        provision_sdk(
            "9.0.308",
            "",
            &root,
            Arch::Arm64,
            &mut out,
            &fake_fetch,
            &install2,
        )
        .unwrap();
        assert!(!ran_again.get(), "an already-installed SDK is reused");
    }

    /// The digest algorithm follows the vendor's flag — .NET publishes SHA-512,
    /// but the wrong branch would verify with the wrong algorithm and reject a
    /// correct download.
    #[test]
    fn tarball_artifact_picks_the_digest_algorithm() {
        use super::{dotnet_tarball_artifact, fetch::Digest, resolve::Install};
        let tarball = |is_512| Install::Tarball {
            url: "https://x/sdk.tar.gz".into(),
            digest_hex: "abc".into(),
            digest_is_sha512: is_512,
            strip_components: 0,
        };
        assert!(matches!(
            dotnet_tarball_artifact(&tarball(true)).unwrap().digest,
            Digest::Sha512(_)
        ));
        assert!(matches!(
            dotnet_tarball_artifact(&tarball(false)).unwrap().digest,
            Digest::Sha256(_)
        ));
        // A non-tarball install (rustup/preprovisioned) is a named error.
        dotnet_tarball_artifact(&Install::Preprovisioned).unwrap_err();
    }

    /// `sdk/<version>` is renamed LAST, so a run interrupted before it leaves no
    /// selectable SDK — a partial that a later run would treat as complete.
    #[test]
    fn a_missing_staged_sdk_is_an_error_and_lands_nothing_selectable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dotnet");
        std::fs::create_dir_all(&root).unwrap();
        let staging = tmp.path().join(".staging");
        // Runtimes staged, but the sdk dir absent (as if the unpack died there).
        write(&staging, "shared/Microsoft.NETCore.App/9.0.0/y.dll", "rt9");

        let err = merge_sdk_into_root(&staging, &root, "9.0.316");
        assert!(err.is_err(), "a missing staged sdk must error");
        assert!(
            !root.join("sdk/9.0.316").exists(),
            "no selectable SDK is left"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the env-var mutation these tests need. `std::env::set_var` is
    /// process-global, so two tests racing on it would see each other's value.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_cache_root<T>(root: Option<&Path>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match root {
            Some(p) => std::env::set_var(CACHE_ROOT_ENV, p),
            None => std::env::remove_var(CACHE_ROOT_ENV),
        }
        let out = f();
        std::env::remove_var(CACHE_ROOT_ENV);
        out
    }

    const GO_JSON: &str = r#"[
      {"version":"go1.24.5","stable":true,"files":[
        {"filename":"go1.24.5.linux-arm64.tar.gz","os":"linux","arch":"arm64",
         "version":"go1.24.5","sha256":"h","size":1,"kind":"archive"},
        {"filename":"go1.24.5.linux-amd64.tar.gz","os":"linux","arch":"amd64",
         "version":"go1.24.5","sha256":"h","size":1,"kind":"archive"}]}
    ]"#;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "must match the fetch_text signature it is passed as"
    )]
    fn go_meta(_: &str) -> Result<String, String> {
        Ok(GO_JSON.to_string())
    }

    fn workspace_with_go_mod(version: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("go.mod"),
            format!("module x\n\ntoolchain go{version}\n"),
        )
        .expect("write");
        tmp
    }

    /// The provisioning signal must reach the consumer BEFORE the download
    /// begins. A first fetch moves hundreds of megabytes; a silent producer for
    /// that window is indistinguishable from a hung one — the same failure the
    /// meta-frame flush fixed on the wire.
    ///
    /// Mutation-checked: moving the `writeln!` after `cache.provision` makes the
    /// installer observe an empty progress buffer and this fails.
    /// A `Write` that mirrors into a shared buffer, so a callback running LATER
    /// can observe exactly what had been emitted by the time it ran.
    #[derive(Clone)]
    struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl SharedSink {
        fn new() -> Self {
            Self(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
        }
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("lock")).into_owned()
        }
    }

    impl Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // A poisoned lock here means a test already failed; recording the
            // bytes anyway keeps the diagnostic readable.
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn progress_is_announced_before_the_download_starts() {
        let ws = workspace_with_go_mod("1.24.5");
        let cache = tempfile::tempdir().expect("tempdir");
        let mut progress = SharedSink::new();
        let observer = progress.clone();
        // What the progress buffer held at the instant the installer ran.
        let seen_at_install = std::sync::Mutex::new(String::new());

        let outcome = with_cache_root(Some(cache.path()), || {
            provision(
                Language::Go,
                ws.path(),
                Arch::Arm64,
                &mut progress,
                &go_meta,
                &|_artifact, staging| {
                    *seen_at_install.lock().expect("lock") = observer.text();
                    std::fs::write(staging.join("bin"), b"go").map_err(|e| FetchError::Io {
                        url: "test".to_string(),
                        source: e,
                    })
                },
            )
        })
        .expect("provision");

        // THE assertion: the announcement was already out when the download ran.
        let at_install = seen_at_install.lock().expect("lock").clone();
        assert!(
            at_install.contains("provisioning go 1.24.5"),
            "the start signal must precede the download, but the installer saw: {at_install:?}"
        );
        assert!(
            at_install.contains("go.mod"),
            "names the pin file: {at_install}"
        );
        // And the completion signal came only afterwards.
        assert!(
            !at_install.contains("provisioned go"),
            "completion must not be announced before it happened: {at_install}"
        );
        assert!(
            progress.text().contains("provisioned go 1.24.5"),
            "{}",
            progress.text()
        );
        assert!(
            matches!(outcome, Outcome::Provisioned { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn an_already_present_toolchain_is_not_downloaded_again() {
        let ws = workspace_with_go_mod("1.24.5");
        let cache_dir = tempfile::tempdir().expect("tempdir");
        // Seeded under the ARCH the call below asks for. An arch-less path here
        // no longer satisfies the lookup, which is the point of the key.
        std::fs::create_dir_all(cache_dir.path().join("arm64/go/1.24.5")).expect("mkdir");
        let mut progress: Vec<u8> = Vec::new();

        let outcome = with_cache_root(Some(cache_dir.path()), || {
            provision(
                Language::Go,
                ws.path(),
                Arch::Arm64,
                &mut progress,
                &go_meta,
                &|_, _| panic!("must not download an already-provisioned toolchain"),
            )
        })
        .expect("provision");

        assert!(
            matches!(outcome, Outcome::AlreadyPresent { .. }),
            "{outcome:?}"
        );
        assert!(
            progress.is_empty(),
            "nothing to announce when nothing happens"
        );
    }

    /// Without the cache mounted there is nowhere to install to. That is not an
    /// error — it means the image is running outside kenn's docker runtime.
    #[test]
    fn no_mounted_cache_is_a_no_op_not_a_failure() {
        let ws = workspace_with_go_mod("1.24.5");
        let mut progress: Vec<u8> = Vec::new();
        let outcome = with_cache_root(None, || {
            provision(
                Language::Go,
                ws.path(),
                Arch::Arm64,
                &mut progress,
                &|_| panic!("must not fetch metadata with no cache"),
                &|_, _| panic!("must not install"),
            )
        })
        .expect("no cache is not an error");
        assert_eq!(outcome, Outcome::NoCache);
    }

    /// An unresolvable pin is FATAL and names the pin and its file. Falling back
    /// to whatever is present is the bug this change exists to remove.
    #[test]
    fn an_unresolvable_pin_fails_and_names_the_pin_and_its_source() {
        let ws = workspace_with_go_mod("1.99.99");
        let cache = tempfile::tempdir().expect("tempdir");
        let mut progress: Vec<u8> = Vec::new();

        let err = with_cache_root(Some(cache.path()), || {
            provision(
                Language::Go,
                ws.path(),
                Arch::Arm64,
                &mut progress,
                &go_meta,
                &|_, _| panic!("must not install when resolution failed"),
            )
        })
        .expect_err("an unmatched pin must be fatal");

        let msg = err.to_string();
        assert!(msg.contains("1.99.99"), "names the pin: {msg}");
        assert!(msg.contains("go.mod"), "names its source: {msg}");
    }

    /// With no pin file, a language with a "latest" default still provisions —
    /// the image carries no toolchain, so doing nothing would leave the indexer
    /// with none at all.
    #[test]
    fn an_unpinned_workspace_falls_back_to_latest() {
        let ws = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("tempdir");
        let mut progress: Vec<u8> = Vec::new();

        let outcome = with_cache_root(Some(cache.path()), || {
            provision(
                Language::Go,
                ws.path(),
                Arch::Arm64,
                &mut progress,
                &go_meta,
                &|_, staging| {
                    std::fs::write(staging.join("bin"), b"go").map_err(|e| FetchError::Io {
                        url: "t".to_string(),
                        source: e,
                    })
                },
            )
        })
        .expect("provision");

        // The cache key must be the CONCRETE version, never the sentinel.
        match outcome {
            Outcome::Provisioned { version, .. } => assert_eq!(version, "1.24.5"),
            other => panic!("expected a provision, got {other:?}"),
        }
        assert!(!cache.path().join("go").join(resolve::LATEST).exists());
    }

    /// rustup installs to `toolchains/<channel>-<triple>/bin` and creates NO
    /// `CARGO_HOME/bin` shims — those come from `rustup-init`, not from
    /// `rustup toolchain install`. Pointing PATH at a shim directory or at
    /// `<root>/bin` finds nothing, and rust-analyzer then fails to run
    /// `cargo metadata` with no indication that a toolchain was ever installed.
    ///
    /// Found by listing the volume after a real provision, not by reasoning.
    #[test]
    fn the_rust_bin_dir_is_inside_the_installed_toolchain() {
        let root = tempfile::tempdir().expect("tempdir");
        let installed = root
            .path()
            .join("toolchains/1.90.0-aarch64-unknown-linux-gnu/bin");
        std::fs::create_dir_all(&installed).expect("mkdir");

        assert_eq!(toolchain_bin(Language::Rust, root.path()), installed);
        // And not either of the plausible-but-wrong answers.
        assert_ne!(
            toolchain_bin(Language::Rust, root.path()),
            root.path().join("bin")
        );
        assert_ne!(
            toolchain_bin(Language::Rust, root.path()),
            root.path().join("cargo/bin")
        );
    }

    /// .NET puts `dotnet` at the root of the SDK tree; everything else uses a
    /// plain `bin/`. Getting either wrong points PATH at a missing directory.
    #[test]
    fn the_other_layouts_are_root_or_bin() {
        let root = Path::new("/tc/x");
        assert_eq!(toolchain_bin(Language::Dotnet, root), root);
        assert_eq!(toolchain_bin(Language::Go, root), root.join("bin"));
        assert_eq!(toolchain_bin(Language::Node, root), root.join("bin"));
        // Swift keeps the /usr prefix from `cp --parents`.
        assert_eq!(toolchain_bin(Language::Swift, root), root.join("usr/bin"));
    }

    /// Swift must refuse rather than provision unverified, and say why.
    #[test]
    fn swift_refuses_with_its_reason() {
        let ws = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            ws.path().join("Package.swift"),
            "// swift-tools-version:6.0\n",
        )
        .expect("write");
        let cache = tempfile::tempdir().expect("tempdir");
        let mut progress: Vec<u8> = Vec::new();

        let err = with_cache_root(Some(cache.path()), || {
            provision(
                Language::Swift,
                ws.path(),
                Arch::Arm64,
                &mut progress,
                &|_| panic!("must not fetch"),
                &|_, _| panic!("must not install"),
            )
        })
        .expect_err("an absent host-provisioned toolchain must fail");
        // Named and actionable: indexing Swift without a toolchain is exactly the
        // silent-zero this change exists to remove.
        let msg = err.to_string();
        assert!(msg.contains("host"), "says where it comes from: {msg}");
        assert!(msg.contains("local"), "offers the way forward: {msg}");
    }
}
