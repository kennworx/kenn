//! Docker-runtime launcher rewrite (docker-indexer-runtime, phase 3).
//!
//! When a language's `runtime = "docker"`, its configured `command` is wrapped
//! into a `docker run` invocation before the driver spawns it. The workspace is
//! bind-mounted **at its own absolute path** (the POSIX same-path mount), so the
//! absolute-path arguments each driver appends after the launcher resolve
//! unchanged inside the container and the emitted output lands on the host. The
//! Windows `/work`-mount-plus-translation path is a separate follow-on behind
//! [`MountStrategy`].

use std::path::{Path, PathBuf};

use kenn_config::Runtime;

/// How the workspace is mounted into the container. Only [`MountStrategy::SamePath`]
/// (POSIX) is implemented; `Translate` (Windows `/work` + path translation) is the
/// follow-on seam and is intentionally absent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MountStrategy {
    /// Bind-mount the workspace at its own absolute host path.
    SamePath,
}

/// The invoking user's `(uid, gid)`, so container-written files (SCIP output,
/// `.kenn/`) are owned by the caller rather than root.
#[cfg(unix)]
#[expect(unsafe_code, reason = "FFI to argument-less, infallible getuid/getgid")]
fn current_ids() -> (u32, u32) {
    // SAFETY: getuid takes no arguments, never fails, is async-signal-safe.
    let uid = unsafe { libc::getuid() };
    // SAFETY: getgid takes no arguments, never fails, is async-signal-safe.
    let gid = unsafe { libc::getgid() };
    (uid, gid)
}

/// Non-unix hosts don't use `--user`; the Windows docker path (the deferred
/// `Translate` mount) is out of scope here.
#[cfg(not(unix))]
fn current_ids() -> (u32, u32) {
    (0, 0)
}

/// Where a language's build artifacts (cargo `target/`, Go build cache) go. A
/// `volume` (per-workspace) persists them across re-indexes; `None` uses an
/// ephemeral in-container dir dropped on `--rm`. Build caches are NEVER the
/// shared dependency-source volume — cargo locks `target/`, so sharing it across
/// repos would stall parallel indexer runs.
pub(crate) struct BuildCache<'a> {
    pub env: &'static str,
    pub subdir: &'static str,
    pub volume: Option<&'a str>,
}

/// A language's docker caches: the dependency-*source* cache (a shared named
/// volume, cross-repo — mac/Windows can't afford a bind-mounted hot cache) and,
/// optionally, a build-artifact cache ([`BuildCache`]).
pub(crate) struct LangCache<'a> {
    pub source_env: &'static str,
    pub source_subdir: &'static str,
    pub source_volume: &'a str,
    pub build: Option<BuildCache<'a>>,
}

/// Wrap `command` into a `docker run` invocation that runs it inside `image`
/// under the [`MountStrategy::SamePath`] mount. The result is the new launcher
/// token vector: the driver still does `Command::new(argv[0])` (`docker`) and
/// appends its absolute-path intrinsic args, which stay valid in the container.
///
/// `cache` mounts the shared dependency-source volume at `/kenn-cache` and points
/// the tool's source cache there, and (when a build cache is requested) mounts a
/// per-workspace volume at `/kenn-build` or falls back to an ephemeral
/// `/tmp/kenn-build`. `None` for languages with no cache to warm.
pub(crate) fn docker_launcher(
    command: &[String],
    image: &str,
    cache: Option<LangCache<'_>>,
    ws_root: &Path,
    _strategy: MountStrategy,
) -> Vec<String> {
    let root = ws_root.display().to_string();
    let (uid, gid) = current_ids();
    let mut argv = vec![
        "docker".to_string(),
        "run".to_string(),
        "--rm".to_string(),
        // Run as the caller so files written into the mount are host-owned.
        "--user".to_string(),
        format!("{uid}:{gid}"),
        // The image can't assume a writable /root under an arbitrary uid.
        "-e".to_string(),
        "HOME=/tmp".to_string(),
        // Same-path mount: the workspace is valid at its own path inside.
        "-v".to_string(),
        format!("{root}:{root}"),
        "-w".to_string(),
        root,
        // The provisioned-toolchain cache, on EVERY docker-runtime language —
        // not conditional on `cache`, because a language with no dependency
        // cache still needs its toolchain. The image's entrypoint provisions
        // into here when the workspace's pinned version is absent.
        "-v".to_string(),
        format!("{TOOLCHAIN_VOLUME}:{TOOLCHAIN_MOUNT}"),
        "-e".to_string(),
        format!("{TOOLCHAIN_ROOT_ENV}={TOOLCHAIN_MOUNT}"),
    ];
    if let Some(c) = cache {
        // Dependency sources → shared named volume (fast on mac/Windows).
        argv.push("-v".to_string());
        argv.push(format!("{}:/kenn-cache", c.source_volume));
        argv.push("-e".to_string());
        argv.push(format!("{}=/kenn-cache/{}", c.source_env, c.source_subdir));
        if let Some(b) = c.build {
            // Build artifacts → per-workspace volume (persisted) or ephemeral.
            let build_root = match b.volume {
                Some(vol) => {
                    argv.push("-v".to_string());
                    argv.push(format!("{vol}:/kenn-build"));
                    "/kenn-build"
                }
                None => "/tmp/kenn-build",
            };
            argv.push("-e".to_string());
            argv.push(format!("{}={build_root}/{}", b.env, b.subdir));
        }
    }
    argv.push(image.to_string());
    argv.extend(command.iter().cloned());
    argv
}

/// The launcher tokens for a language: the raw `command` when `runtime = Local`,
/// or the [`docker_launcher`] wrapping when `runtime = Docker` with an image.
/// A validated config guarantees a docker runtime carries an image; a missing
/// one falls back to the raw command rather than producing a broken invocation.
pub(crate) fn maybe_docker_command(
    command: &[String],
    runtime: Runtime,
    image: Option<&str>,
    cache: Option<LangCache<'_>>,
    ws_root: &Path,
) -> Vec<String> {
    match (runtime, image) {
        (Runtime::Docker, Some(img)) if !img.is_empty() => {
            docker_launcher(command, img, cache, ws_root, MountStrategy::SamePath)
        }
        _ => command.to_vec(),
    }
}

/// Probe that the Docker daemon is responding, for the phase-1 preflight when
/// any language runs `runtime = "docker"`. The PATH check already catches a
/// missing `docker` binary; this catches an installed-but-not-running daemon.
#[must_use]
pub fn daemon_available() -> bool {
    use std::process::{Command, Stdio};
    Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Docker label on every kenn-created cache volume — the **enumeration** key, by
/// which `kenn docker-cache` finds kenn's volumes (`--filter label=kenn.managed`).
pub const LABEL_MANAGED: &str = "kenn.managed";
/// Docker label on a **bound** cache volume (a build volume → its worktree, a
/// per-repo deps volume → the main worktree), holding the absolute bound
/// directory — the **orphan-binding** key. Absent on a configured shared
/// cross-repo volume, which is therefore never an orphan.
pub const LABEL_WORKSPACE: &str = "kenn.workspace";

/// A named Docker cache volume the preflight creates + labels. `bound_dir` is the
/// absolute directory this volume is tied to (a worktree for a build volume, the
/// repo's main worktree for a per-repo deps volume); `None` for a configured
/// shared cross-repo volume that is never reclaimed automatically.
#[derive(Debug, Clone)]
pub struct CacheVolume {
    pub name: String,
    pub bound_dir: Option<PathBuf>,
}

/// Stable 16-hex digest of `dir`'s absolute path — the volume-name suffix, and
/// the one point both the indexer (creating a volume) and `kenn docker-cache`
/// (removing one) hash, so their names always agree. Canonicalizes internally
/// (idempotent on an already-canonical path).
fn dir_hash(dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The per-worktree build-artifact volume name for `worktree`, stable across
/// re-indexes of the same worktree.
#[must_use]
pub fn build_volume_name(worktree: &Path) -> String {
    format!("kenn-build-{}", dir_hash(worktree))
}

/// The per-repository dependency-source volume name, bound to the repo's
/// `main_worktree` and shared by all of that repo's worktrees.
#[must_use]
pub fn deps_volume_name(main_worktree: &Path) -> String {
    format!("kenn-deps-{}", dir_hash(main_worktree))
}

/// The machine-wide provisioned-toolchain volume. Unlike the build and
/// dependency volumes it carries no directory hash: a toolchain belongs to the
/// machine, not to a repository, so every workspace shares one copy. Being
/// unbound also means `--orphans` never reaps it — see the reclaim command,
/// which targets it explicitly instead.
pub const TOOLCHAIN_VOLUME: &str = "kenn-toolchains";

/// Where [`TOOLCHAIN_VOLUME`] is mounted inside every indexer container, and the
/// env var the entrypoint reads to find it.
const TOOLCHAIN_MOUNT: &str = "/kenn-toolchains";
pub const TOOLCHAIN_ROOT_ENV: &str = "KENN_TOOLCHAIN_ROOT";

/// The toolchain volume as a [`CacheVolume`], for preflight creation. Bound to
/// no directory, so it is labeled `kenn.managed` without `kenn.workspace`.
#[must_use]
pub fn toolchain_volume() -> CacheVolume {
    CacheVolume {
        name: TOOLCHAIN_VOLUME.to_string(),
        bound_dir: None,
    }
}

/// Create the cache `vol` (idempotent), label it so `kenn docker-cache` can find
/// and reason about it, and chown its mount point to the invoking user so a
/// `--user <uid>` container can write it (a fresh named volume is root-owned).
/// Run once before the docker indexers, in preflight. `busybox` is the tiny,
/// universal image used only to chown. `docker volume create` is idempotent and
/// does NOT update labels on an existing volume, so labels are write-once at
/// first creation — fine, since a volume's binding never changes.
pub(crate) fn ensure_cache_volume(vol: &CacheVolume) -> Result<(), String> {
    // Every kenn volume carries `kenn.managed`; a bound volume also carries
    // `kenn.workspace=<dir>` so `--orphans` can test that directory's existence.
    let mut create = vec![
        "volume".to_string(),
        "create".to_string(),
        "--label".to_string(),
        format!("{LABEL_MANAGED}=true"),
    ];
    if let Some(dir) = &vol.bound_dir {
        create.push("--label".to_string());
        create.push(format!("{LABEL_WORKSPACE}={}", dir.display()));
    }
    create.push(vol.name.clone());
    run_docker_checked(&create, &format!("docker volume create {}", vol.name))?;

    // Fresh named volumes are root-owned; chown so a `--user <uid>` container can
    // write. `busybox` is the tiny, universal image used only to chown.
    let (uid, gid) = current_ids();
    let chown = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/v", vol.name),
        "busybox".to_string(),
        "chown".to_string(),
        format!("{uid}:{gid}"),
        "/v".to_string(),
    ];
    run_docker_checked(&chown, &format!("chown cache volume {}", vol.name))
}

/// Run `docker <args>`, discarding stdout and mapping a spawn failure or non-zero
/// exit into an error string prefixed with `context` (with captured stderr).
fn run_docker_checked(args: &[String], context: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let out = Command::new("docker")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{context}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{context}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The kind of a kenn cache volume, read from its name prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    Build,
    Deps,
    /// The machine-wide provisioned-toolchain volume. Exactly one exists.
    Toolchain,
    Other,
}

impl VolumeKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            VolumeKind::Build => "build",
            VolumeKind::Deps => "deps",
            VolumeKind::Toolchain => "toolchain",
            VolumeKind::Other => "other",
        }
    }
}

/// Classify a volume by its name: the `kenn-build-`/`kenn-deps-` prefixes, or
/// the exact `kenn-toolchains` name (it has no hash suffix, being unbound).
#[must_use]
pub fn volume_kind(name: &str) -> VolumeKind {
    if name.starts_with("kenn-build-") {
        VolumeKind::Build
    } else if name.starts_with("kenn-deps-") {
        VolumeKind::Deps
    } else if name == TOOLCHAIN_VOLUME {
        VolumeKind::Toolchain
    } else {
        VolumeKind::Other
    }
}

/// One provisioned toolchain inside [`TOOLCHAIN_VOLUME`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedToolchain {
    /// The architecture segment. The cache holds one tree per arch, so a
    /// mixed-arch host legitimately shows the same language+version twice.
    pub arch: String,
    pub language: String,
    pub version: String,
    pub size_kb: u64,
}

/// Names that may be interpolated into a helper container's argv. A toolchain
/// language/version reaches us from the CLI, so it is restricted to characters
/// that cannot mean anything to a shell or escape a path segment.
fn is_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && s != "."
        && s != ".."
}

/// The toolchains provisioned in the shared volume, with their sizes.
///
/// The volume's contents are only reachable through a container: on macOS and
/// Windows the mountpoint `docker volume inspect` reports lives inside the VM,
/// not on the host. `busybox` is the same tiny helper image
/// [`ensure_cache_volume`] already uses.
///
/// Returns an empty list when the volume does not exist — nothing provisioned
/// yet is not an error.
pub fn list_toolchains() -> Result<Vec<ProvisionedToolchain>, String> {
    use std::process::Command;
    // `du -sk <root>/*/*/*` — one line per `<arch>/<language>/<version>`
    // directory. The glob skips `.staging` for free, since sh globs do not match
    // dotfiles.
    let out = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{TOOLCHAIN_VOLUME}:/t"),
            "busybox",
            "sh",
            "-c",
            // BOTH depths: `<arch>/<language>/<version>` is the current layout,
            // and `<language>/<version>` is what caches written before the arch
            // key look like. Listing only the new depth would leave those
            // invisible AND unreachable — gigabytes a user cannot see or
            // reclaim. `parse_du` tells them apart.
            //
            // TWO invocations, not one with both globs: `du` does not re-count
            // inodes it has already seen in a single run, so the shallower glob
            // summarizes `<arch>/<language>` and the deeper one then reports
            // NOTHING for the versions beneath it. That silently dropped every
            // current-layout toolchain from the listing while looking correct.
            "du -sk /t/*/*/* 2>/dev/null; du -sk /t/*/* 2>/dev/null",
        ])
        .output()
        .map_err(|e| format!("listing toolchains: {e}"))?;
    // A missing volume, or an empty one, is simply nothing provisioned.
    Ok(parse_du(&String::from_utf8_lossy(&out.stdout)))
}

/// The architecture segments the cache is keyed by. Used to tell a current
/// `<arch>/<language>` directory from a pre-arch `<language>/<version>` one,
/// since both are two segments deep and `du` reports them identically.
const ARCH_SEGMENTS: &[&str] = &["amd64", "arm64"];

/// Marks a toolchain from a cache written before the arch key existed. Nothing
/// reads these any more; they are surfaced only so the space can be reclaimed.
pub const LEGACY_ARCH: &str = "legacy";

/// Parse `du -sk` output lines. Two shapes arrive, because the listing globs
/// both depths (see [`list_toolchains`]):
///
/// - `<kb>\t/t/<arch>/<language>/<version>` — current.
/// - `<kb>\t/t/<language>/<version>` — pre-arch, reported as [`LEGACY_ARCH`].
///
/// The two-segment glob also matches `<arch>/<language>` intermediate dirs,
/// whose children the three-segment glob already reported. Those are dropped,
/// or every toolchain would be counted twice — once truly, once as a phantom
/// `language=<arch>, version=<language>`.
fn parse_du(stdout: &str) -> Vec<ProvisionedToolchain> {
    stdout
        .lines()
        .filter_map(|line| {
            let (size, path) = line.split_once('\t')?;
            let rest = path.trim_end_matches('/').strip_prefix("/t/")?;
            let parts: Vec<&str> = rest.split('/').collect();
            // The leading segment must be a known arch for a three-segment path
            // to be a toolchain. Without that check the three-segment glob
            // descends INTO a pre-arch tree and reports its contents as
            // toolchains — `/t/dotnet/10.0.302/LICENSE.txt` became
            // `arch=dotnet, language=10.0.302, version=LICENSE.txt`, one bogus
            // row per file.
            // Two arms reject with the same body for DIFFERENT reasons — a file
            // inside a legacy tree, and an `<arch>/<language>` intermediate dir
            // — and each is the subject of its own test. Collapsing them to
            // satisfy the lint would erase which case is which.
            #[expect(
                clippy::match_same_arms,
                reason = "distinct rejections, separately tested"
            )]
            let (arch, language, version) = match parts.as_slice() {
                [arch, language, version] if ARCH_SEGMENTS.contains(arch) => {
                    (*arch, *language, *version)
                }
                [_, _, _] => return None,
                [first, _] if ARCH_SEGMENTS.contains(first) => return None,
                [language, version] => (LEGACY_ARCH, *language, *version),
                _ => return None,
            };
            Some(ProvisionedToolchain {
                arch: arch.to_string(),
                language: language.to_string(),
                version: version.to_string(),
                size_kb: size.trim().parse().ok()?,
            })
        })
        .collect()
}

/// The path to remove inside the toolchain volume, or `None` when either
/// segment is unsafe.
///
/// Split out from [`remove_toolchain`] so the validation is reachable from a
/// test: the caller interpolates these segments into a `sh -c` command line, so
/// a `..` or a shell metacharacter reaching this point is an escape out of the
/// volume, not a cosmetic issue.
/// Every arch's copy is a target, plus the pre-arch location: the CLI names a
/// language and optionally a version (`--toolchain go@1.26.5`) and never an
/// arch, so "remove go 1.26.5" must mean all of them. Removing only the host's
/// would leave the others as invisible, unreclaimable space.
fn toolchain_target(language: &str, version: Option<&str>) -> Option<Vec<String>> {
    if !is_safe_segment(language) || version.is_some_and(|v| !is_safe_segment(v)) {
        return None;
    }
    let suffix = match version {
        Some(v) => format!("{language}/{v}"),
        None => language.to_string(),
    };
    let mut targets: Vec<String> = ARCH_SEGMENTS
        .iter()
        .map(|arch| format!("/t/{arch}/{suffix}"))
        .collect();
    targets.push(format!("/t/{suffix}"));
    Some(targets)
}

/// Remove one provisioned toolchain from the shared volume — a whole language
/// when `version` is `None`, otherwise just that version. The volume itself
/// survives, so other workspaces keep their toolchains.
#[must_use]
pub fn remove_toolchain(language: &str, version: Option<&str>) -> RemoveOutcome {
    use std::process::Command;
    let Some(targets) = toolchain_target(language, version) else {
        return RemoveOutcome::Failed("invalid toolchain name".to_string());
    };
    // Removes each candidate that exists and succeeds only if at least one did,
    // so "nothing matched" is still distinguishable from "removed" — the guard
    // `test -e` used to give for a single path.
    let script = format!(
        "found=0; for p in {}; do if [ -e \"$p\" ]; then rm -r \"$p\" && found=1; fi; done; \
         [ \"$found\" = 1 ]",
        targets.join(" ")
    );
    let out = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{TOOLCHAIN_VOLUME}:/t"),
            "busybox",
            "sh",
            "-c",
            &script,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => RemoveOutcome::Removed,
        Ok(_) => RemoveOutcome::NotFound,
        Err(e) => RemoveOutcome::Failed(format!("removing {language}: {e}")),
    }
}

/// The directories that make up a Swift toolchain inside the official image.
/// Scattered rather than under one prefix, so each moves separately.
const SWIFT_TOOLCHAIN_PATHS: &[&str] = &[
    "/usr/lib/swift",
    "/usr/libexec/swift",
    "/usr/share/swift",
    // clang's RESOURCE headers (stddef.h, stdarg.h, …). Compiler-provided rather
    // than libc, so `libc6-dev` does not supply them. Without this directory any
    // target with a C interop shim fails at "'stddef.h' file not found", which
    // reads as a broken target rather than an incomplete toolchain copy.
    "/usr/lib/clang",
];

/// Name prefixes of the toolchain's own executables in `/usr/bin`.
///
/// `/usr/bin` is NOT copied wholesale. The official image is ubuntu-based and
/// that directory holds 507 entries, only 49 of which are the toolchain — the
/// rest are the distro's own userland. Copying all of it and prepending the
/// result to `PATH` shadows the *indexer* image's coreutils with ubuntu's, and
/// they then fail on a glibc version mismatch: measured, `head` died with
/// "`GLIBC_2.38` not found" and the real error was never seen.
const SWIFT_BIN_PREFIXES: &[&str] = &[
    "swift",
    "clang",
    "llvm",
    "lld",
    "lldb",
    "sourcekit",
    "ld.",
    "wasm",
    "dsymutil",
    "llc",
    "opt",
];

/// Populate the toolchain cache with a Swift toolchain taken from the official
/// `swift:<tag>` image.
///
/// # Why Swift alone is provisioned from an image
///
/// Every other language publishes a tarball with a checksum we verify. swift.org
/// publishes neither for Linux toolchains — only a detached PGP signature — so
/// there is nothing to verify a download against. A container image sidesteps
/// that entirely: registry content is addressed by digest and every layer is
/// verified on pull, which is a stronger guarantee than a hash we fetch over the
/// same connection as the artifact.
///
/// # Why this runs on the host and not in the entrypoint
///
/// The entrypoint cannot call docker — that would need docker-in-docker. So this
/// is the one provisioning path that lives on this side of the boundary. It is
/// still docker doing the fetching and the verifying; kenn only names the image.
///
/// Verified: the toolchain relocates. Compiled and ran a program from a copy at
/// a different prefix with the original `/usr/lib/swift` moved away.
///
/// # What the runtime image owes a provisioned Swift toolchain
///
/// Discovered by running it until it worked, one error at a time — none of these
/// are guessable, and each failure named only the next missing piece:
///
/// - **glibc >= the toolchain's build distro.** `swift:*-noble` needs
///   `GLIBC_2.38`, so the base must be ubuntu 24.04 (2.39) or newer — this is
///   why every indexer image is noble, and it must be bumped if the official
///   Swift images move to a newer ubuntu.
/// - `libncurses6 libxml2 libcurl4 zlib1g libsqlite3-0 libedit2 libpython3.12`
///   — linked by `swift-frontend`.
/// - `libc6-dev` and **`gcc`** — the LINK step needs `Scrt1.o` from the former
///   and `crtbeginS.o`/`crtendS.o` from the latter. Without them `swiftc`
///   type-checks fine and fails only at link, which reads as a project error
///   rather than a missing payload.
pub fn provision_swift_from_image(image: &str, dest_version: &str) -> Result<(), String> {
    use std::process::Command;
    if !is_safe_segment(dest_version) {
        return Err(format!("invalid swift version {dest_version:?}"));
    }
    // Arch-scoped like every entrypoint-written install. The image is pulled for
    // the host platform, so the toolchain copied out of it is the host's — and
    // writing it to an arch-blind path is what let an amd64 container pick up an
    // arm64 toolchain.
    let arch = kenn_toolchain::resolve::Arch::host().cache_key();
    let dest = format!("/t/{arch}/swift/{dest_version}");
    // Staged and renamed, exactly like the entrypoint's installs: a partial copy
    // must never be visible as a complete toolchain.
    let staging = format!("{dest}.staging");
    let bin_globs: Vec<String> = SWIFT_BIN_PREFIXES
        .iter()
        .map(|p| format!("/usr/bin/{p}*"))
        .collect();
    let script = format!(
        "set -e; rm -rf {staging} {dest}; mkdir -p {staging}/usr/bin; \
         for p in {paths}; do [ -e \"$p\" ] && cp -a --parents \"$p\" {staging} || true; done; \
         for g in {globs}; do cp -a $g {staging}/usr/bin/ 2>/dev/null || true; done; \
         test -x {staging}/usr/bin/swiftc; \
         mv {staging} {dest}",
        paths = SWIFT_TOOLCHAIN_PATHS.join(" "),
        globs = bin_globs.join(" "),
    );
    let out = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{TOOLCHAIN_VOLUME}:/t"),
            image,
            "sh",
            "-c",
            &script,
        ])
        .output()
        .map_err(|e| format!("provisioning swift from {image}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "provisioning swift from {image}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// A kenn-managed Docker cache volume, as reported by [`list_managed_volumes`].
#[derive(Debug, Clone)]
pub struct ManagedVolume {
    pub name: String,
    pub kind: VolumeKind,
    /// The `kenn.workspace` bound directory, or `None` for a shared (unbound)
    /// volume that `--orphans` never reaps.
    pub bound_dir: Option<PathBuf>,
    pub in_use: bool,
}

impl ManagedVolume {
    /// A bound volume whose directory no longer exists — an orphan. A shared
    /// (unbound) volume is never an orphan.
    #[must_use]
    pub fn is_orphan(&self) -> bool {
        self.bound_dir.as_ref().is_some_and(|d| !d.exists())
    }
}

/// Outcome of removing one volume.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    /// The volume did not exist — removal is idempotent, so this is success.
    NotFound,
    /// A container still references the volume; skipped rather than aborting.
    InUse,
    /// An unexpected Docker failure (permissions, daemon error).
    Failed(String),
}

/// Classify `docker volume rm`'s result from its exit success + stderr, so an
/// absent or in-use volume is a reported non-error and only a genuine failure
/// escalates. Pure, so it is unit-tested against the real Docker messages.
fn classify_rm(success: bool, stderr: &str) -> RemoveOutcome {
    let lower = stderr.to_ascii_lowercase();
    if success {
        RemoveOutcome::Removed
    } else if lower.contains("no such volume") {
        RemoveOutcome::NotFound
    } else if lower.contains("volume is in use") || lower.contains("in use") {
        RemoveOutcome::InUse
    } else {
        RemoveOutcome::Failed(stderr.trim().to_string())
    }
}

/// List every kenn-managed cache volume (`--filter label=kenn.managed`), reading
/// each one's `kenn.workspace` binding and in-use state. Requires a live daemon.
pub fn list_managed_volumes() -> Result<Vec<ManagedVolume>, String> {
    use std::process::Command;
    let out = Command::new("docker")
        .args([
            "volume",
            "ls",
            "--filter",
            &format!("label={LABEL_MANAGED}"),
            "--format",
            "{{.Name}}",
        ])
        .output()
        .map_err(|e| format!("docker volume ls: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "docker volume ls: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut names: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(ToString::to_string)
        .collect();
    // The toolchain volume is included BY NAME, not by label. Labels are
    // write-once at creation (`docker volume create` will not update an existing
    // volume's), and this volume has a fixed, well-known name that anything can
    // auto-create just by mounting it — a bare `docker run -v kenn-toolchains:…`
    // does. When that happens first, the volume exists unlabelled forever, and
    // filtering on the label alone hides the LARGEST thing kenn puts on disk
    // behind a listing that says everything is fine.
    if !names.iter().any(|n| n == TOOLCHAIN_VOLUME) && toolchain_volume_exists() {
        names.push(TOOLCHAIN_VOLUME.to_string());
    }
    let vols = names
        .into_iter()
        .map(|name| ManagedVolume {
            kind: volume_kind(&name),
            bound_dir: volume_bound_dir(&name),
            in_use: volume_in_use(&name),
            name,
        })
        .collect();
    Ok(vols)
}

/// Whether the shared toolchain volume exists at all, labelled or not.
fn toolchain_volume_exists() -> bool {
    use std::process::{Command, Stdio};
    Command::new("docker")
        .args(["volume", "inspect", TOOLCHAIN_VOLUME])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The `kenn.workspace` bound directory of `name`, or `None` when the label is
/// absent (a shared volume) or unreadable.
fn volume_bound_dir(name: &str) -> Option<PathBuf> {
    use std::process::Command;
    let out = Command::new("docker")
        .args([
            "volume",
            "inspect",
            "--format",
            &format!("{{{{ index .Labels \"{LABEL_WORKSPACE}\" }}}}"),
            name,
        ])
        .output()
        .ok()?;
    let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // A missing label prints empty (or `<no value>` on older Docker).
    if !out.status.success() || val.is_empty() || val == "<no value>" {
        None
    } else {
        Some(PathBuf::from(val))
    }
}

/// Whether any container (running or stopped) still references `name`.
fn volume_in_use(name: &str) -> bool {
    use std::process::Command;
    Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("volume={name}"),
            "--format",
            "{{.ID}}",
        ])
        .output()
        .is_ok_and(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
}

/// Remove one volume, classifying the outcome (idempotent + in-use-safe).
#[must_use]
pub fn remove_volume(name: &str) -> RemoveOutcome {
    use std::process::Command;
    match Command::new("docker").args(["volume", "rm", name]).output() {
        Ok(o) => classify_rm(o.status.success(), &String::from_utf8_lossy(&o.stderr)),
        Err(e) => RemoveOutcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_flags<'a>(argv: &'a [String], flag: &str) -> Vec<&'a str> {
        argv.iter()
            .enumerate()
            .filter(|(_, a)| *a == flag)
            .filter_map(|(i, _)| argv.get(i + 1))
            .map(String::as_str)
            .collect()
    }

    fn rust_cache(build_volume: Option<&str>) -> LangCache<'_> {
        LangCache {
            source_env: "CARGO_HOME",
            source_subdir: "cargo",
            source_volume: "kenn-docker-cache",
            build: Some(BuildCache {
                env: "CARGO_TARGET_DIR",
                subdir: "cargo",
                volume: build_volume,
            }),
        }
    }

    #[test]
    fn docker_launcher_shared_source_volume_and_ephemeral_build() {
        let argv = docker_launcher(
            &["rust-analyzer".to_string()],
            "ghcr.io/kenn/ra@sha256:abc",
            Some(rust_cache(None)),
            Path::new("/ws/repo"),
            MountStrategy::SamePath,
        );
        assert_eq!(&argv[0..3], &["docker", "run", "--rm"]);
        // Same-path mount + workdir, plus the shared dependency-source volume.
        let mounts = find_flags(&argv, "-v");
        assert!(mounts.contains(&"/ws/repo:/ws/repo"), "{mounts:?}");
        assert!(
            mounts.contains(&"kenn-docker-cache:/kenn-cache"),
            "{mounts:?}"
        );
        assert_eq!(find_flags(&argv, "-w"), vec!["/ws/repo"]);
        assert!(argv.iter().any(|a| a == "--user"));
        // Dependency sources in the shared volume; build artifacts ephemeral.
        let envs = find_flags(&argv, "-e");
        assert!(envs.contains(&"HOME=/tmp"), "{envs:?}");
        assert!(envs.contains(&"CARGO_HOME=/kenn-cache/cargo"), "{envs:?}");
        assert!(
            envs.contains(&"CARGO_TARGET_DIR=/tmp/kenn-build/cargo"),
            "ephemeral build dir: {envs:?}"
        );
        // No build volume mounted when ephemeral.
        assert!(
            !mounts.iter().any(|m| m.contains("/kenn-build")),
            "{mounts:?}"
        );
        // Image then original launcher, at the end.
        let img = argv
            .iter()
            .position(|a| a == "ghcr.io/kenn/ra@sha256:abc")
            .unwrap();
        assert_eq!(argv[img + 1], "rust-analyzer");
        assert_eq!(argv.last().unwrap(), "rust-analyzer");
    }

    /// The toolchain cache rides on EVERY docker-runtime language, including
    /// one with no dependency cache at all — the entrypoint provisions into it
    /// regardless, so a language without a `LangCache` must still get it.
    #[test]
    fn docker_launcher_always_mounts_the_toolchain_volume() {
        for cache in [None, Some(rust_cache(None))] {
            let argv = docker_launcher(
                &["indexer".to_string()],
                "img@sha256:abc",
                cache,
                Path::new("/ws/repo"),
                MountStrategy::SamePath,
            );
            let mounts = find_flags(&argv, "-v");
            assert!(
                mounts.contains(&"kenn-toolchains:/kenn-toolchains"),
                "{mounts:?}"
            );
            let envs = find_flags(&argv, "-e");
            assert!(
                envs.contains(&"KENN_TOOLCHAIN_ROOT=/kenn-toolchains"),
                "{envs:?}"
            );
        }
    }

    /// The version reaches this from a workspace file and is interpolated into a
    /// container's shell alongside `rm -rf`. Anything that could break out of the
    /// path segment must be refused before it gets there.
    ///
    /// Mutation-checked: dropping the `is_safe_segment` guard accepts `..` and
    /// shell metacharacters into a command containing `rm -rf`.
    #[test]
    fn a_swift_version_that_could_escape_is_refused() {
        for bad in ["..", "", "a/b", "a;rm -rf /", "$(id)", "a b"] {
            assert!(
                provision_swift_from_image("swift:6.1", bad).is_err(),
                "must refuse {bad:?}"
            );
        }
    }

    /// The toolchain directories move; `/usr/bin` deliberately does NOT, because
    /// it is 507 entries of ubuntu userland around 49 toolchain binaries.
    /// Copying it wholesale and prepending it to PATH shadowed the indexer
    /// image's coreutils and they died on a glibc mismatch.
    #[test]
    fn the_swift_toolchain_paths_cover_the_scattered_layout() {
        for expected in ["/usr/lib/swift", "/usr/libexec/swift", "/usr/share/swift"] {
            assert!(
                SWIFT_TOOLCHAIN_PATHS.contains(&expected),
                "missing {expected}"
            );
        }
        assert!(
            !SWIFT_TOOLCHAIN_PATHS.contains(&"/usr/bin"),
            "/usr/bin must be filtered by prefix, never copied wholesale"
        );
    }

    #[test]
    fn parse_du_reads_arch_language_version_and_size() {
        let got = parse_du("423616\t/t/arm64/dotnet/9.0.308\n612344\t/t/amd64/swift/5.10.1\n");
        assert_eq!(
            got,
            vec![
                ProvisionedToolchain {
                    arch: "arm64".to_string(),
                    language: "dotnet".to_string(),
                    version: "9.0.308".to_string(),
                    size_kb: 423_616,
                },
                ProvisionedToolchain {
                    arch: "amd64".to_string(),
                    language: "swift".to_string(),
                    version: "5.10.1".to_string(),
                    size_kb: 612_344,
                },
            ]
        );
        // An empty volume, and du's noise on a missing path, yield nothing.
        assert!(parse_du("").is_empty());
        assert!(parse_du("du: /t/*/*: No such file or directory\n").is_empty());
    }

    /// The listing globs two depths, so `<arch>/<language>` intermediate dirs
    /// arrive alongside the real entries. Counting one would double every
    /// toolchain — once truly, once as `language=arm64, version=go`.
    #[test]
    fn an_arch_directory_is_not_mistaken_for_a_toolchain() {
        let got = parse_du("900\t/t/arm64/go\n900\t/t/arm64/go/1.26.5\n");
        assert_eq!(got.len(), 1, "got {got:?}");
        assert_eq!(got[0].language, "go");
        assert_eq!(got[0].version, "1.26.5");
        assert_eq!(got[0].arch, "arm64");
    }

    /// A cache written before the arch key is unreachable but still occupies the
    /// volume. It must stay VISIBLE so the space can be reclaimed — dropping it
    /// from the listing would hide gigabytes rather than free them.
    #[test]
    fn a_pre_arch_entry_is_reported_as_legacy() {
        let got = parse_du("500\t/t/go/1.24.5\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].arch, LEGACY_ARCH);
        assert_eq!(got[0].language, "go");
        assert_eq!(got[0].version, "1.24.5");
    }

    /// The three-segment glob also descends INTO a pre-arch tree, where the
    /// paths are the same depth as a real `<arch>/<language>/<version>` but mean
    /// something else entirely. Observed against a live volume: every file under
    /// a legacy toolchain became its own row —
    /// `arch=dotnet, language=10.0.302, version=LICENSE.txt`.
    #[test]
    fn files_inside_a_legacy_tree_are_not_toolchains() {
        let got = parse_du(
            "4\t/t/dotnet/10.0.302/LICENSE.txt\n\
             72\t/t/dotnet/10.0.302/dotnet\n\
             289272\t/t/dotnet/10.0.302\n\
             270648\t/t/arm64/go/1.26.5\n",
        );
        assert_eq!(got.len(), 2, "got {got:?}");
        assert_eq!(
            (got[0].arch.as_str(), got[0].language.as_str()),
            (LEGACY_ARCH, "dotnet")
        );
        assert_eq!(
            (got[1].arch.as_str(), got[1].language.as_str()),
            ("arm64", "go")
        );
    }

    /// A language/version reaches `remove_toolchain` from the command line and is
    /// interpolated into a helper container's shell. Anything that could break
    /// out of the path segment must be refused before it gets there.
    ///
    /// Mutation-checked: dropping the `is_safe_segment` guard accepts
    /// `..`, `;`, and `$(…)` inputs.
    #[test]
    fn toolchain_names_that_could_escape_are_refused() {
        for bad in [
            "..",
            ".",
            "",
            "a/b",
            "a b",
            "a;rm -rf /",
            "$(id)",
            "`id`",
            "a&b",
            "a|b",
            "a\nb",
            "../../etc",
        ] {
            assert!(!is_safe_segment(bad), "must reject {bad:?}");
            assert!(
                matches!(remove_toolchain(bad, None), RemoveOutcome::Failed(_)),
                "must refuse language {bad:?}"
            );
            assert!(
                matches!(
                    remove_toolchain("dotnet", Some(bad)),
                    RemoveOutcome::Failed(_)
                ),
                "must refuse version {bad:?}"
            );
        }
        for good in ["dotnet", "9.0.308", "1.83.0", "go", "node_22", "swift-6"] {
            assert!(is_safe_segment(good), "must accept {good:?}");
        }
    }

    /// It is machine-wide, so it carries no directory hash and binds to nothing
    /// — which is what keeps `--orphans` from ever reaping it.
    #[test]
    fn the_toolchain_volume_is_unbound_and_classified() {
        let vol = toolchain_volume();
        assert_eq!(vol.name, "kenn-toolchains");
        assert!(vol.bound_dir.is_none(), "must bind to no directory");
        assert_eq!(volume_kind(&vol.name), VolumeKind::Toolchain);
        assert_eq!(VolumeKind::Toolchain.label(), "toolchain");
        // Still distinct from the hashed kinds.
        assert_eq!(volume_kind("kenn-deps-0123456789abcdef"), VolumeKind::Deps);
        assert_eq!(
            volume_kind("kenn-build-0123456789abcdef"),
            VolumeKind::Build
        );
    }

    #[test]
    fn docker_launcher_persists_build_in_a_per_workspace_volume() {
        let argv = docker_launcher(
            &["rust-analyzer".to_string()],
            "img@sha256:a",
            Some(rust_cache(Some("kenn-build-abc123"))),
            Path::new("/ws/repo"),
            MountStrategy::SamePath,
        );
        let mounts = find_flags(&argv, "-v");
        // Build artifacts now ride a per-workspace volume, not /tmp.
        assert!(
            mounts.contains(&"kenn-build-abc123:/kenn-build"),
            "{mounts:?}"
        );
        assert!(
            find_flags(&argv, "-e").contains(&"CARGO_TARGET_DIR=/kenn-build/cargo"),
            "{argv:?}"
        );
    }

    #[test]
    fn maybe_docker_command_passes_local_through_and_wraps_docker() {
        let cmd = vec!["rust-analyzer".to_string()];

        // Local → unchanged.
        assert_eq!(
            maybe_docker_command(
                &cmd,
                Runtime::Local,
                None,
                Some(rust_cache(None)),
                Path::new("/ws")
            ),
            cmd
        );
        // Docker without an image (shouldn't happen post-validate) → unchanged.
        assert_eq!(
            maybe_docker_command(
                &cmd,
                Runtime::Docker,
                None,
                Some(rust_cache(None)),
                Path::new("/ws")
            ),
            cmd
        );
        // Docker + image → wrapped, original launcher preserved at the end.
        let wrapped = maybe_docker_command(
            &cmd,
            Runtime::Docker,
            Some("img@sha256:a"),
            Some(rust_cache(None)),
            Path::new("/ws"),
        );
        assert_eq!(wrapped[0], "docker");
        assert_eq!(wrapped.last().unwrap(), "rust-analyzer");
    }

    #[test]
    fn volume_names_are_deterministic_and_kind_prefixed() {
        let dir = Path::new("/ws/repo");
        // Deterministic, and each kind has its own prefix.
        assert_eq!(build_volume_name(dir), build_volume_name(dir));
        assert!(build_volume_name(dir).starts_with("kenn-build-"));
        assert!(deps_volume_name(dir).starts_with("kenn-deps-"));
        // Both kinds hash the SAME dir to the SAME suffix — the single shared
        // function guarantees the indexer and the cleanup command agree.
        assert_eq!(
            build_volume_name(dir).trim_start_matches("kenn-build-"),
            deps_volume_name(dir).trim_start_matches("kenn-deps-"),
        );
        // Distinct dirs → distinct names.
        assert_ne!(
            build_volume_name(dir),
            build_volume_name(Path::new("/ws/other"))
        );
    }

    #[test]
    fn volume_kind_from_prefix() {
        assert_eq!(volume_kind("kenn-build-abc"), VolumeKind::Build);
        assert_eq!(volume_kind("kenn-deps-abc"), VolumeKind::Deps);
        assert_eq!(volume_kind("kenn-shared"), VolumeKind::Other);
        assert_eq!(volume_kind("random"), VolumeKind::Other);
    }

    #[test]
    fn classify_rm_maps_docker_messages() {
        assert_eq!(classify_rm(true, ""), RemoveOutcome::Removed);
        assert_eq!(
            classify_rm(false, "Error: No such volume: kenn-build-x"),
            RemoveOutcome::NotFound
        );
        assert_eq!(
            classify_rm(
                false,
                "Error response from daemon: remove kenn-deps-x: volume is in use - [abc]"
            ),
            RemoveOutcome::InUse
        );
        assert_eq!(
            classify_rm(false, "permission denied"),
            RemoveOutcome::Failed("permission denied".to_string())
        );
    }

    #[test]
    fn is_orphan_only_for_a_missing_bound_dir() {
        let live = ManagedVolume {
            name: "kenn-build-a".into(),
            kind: VolumeKind::Build,
            bound_dir: Some(std::env::temp_dir()),
            in_use: false,
        };
        assert!(!live.is_orphan(), "live dir is not an orphan");
        let gone = ManagedVolume {
            name: "kenn-build-b".into(),
            kind: VolumeKind::Build,
            bound_dir: Some(PathBuf::from("/no/such/kenn/dir/xyz")),
            in_use: false,
        };
        assert!(gone.is_orphan(), "missing dir is an orphan");
        let shared = ManagedVolume {
            name: "kenn-shared".into(),
            kind: VolumeKind::Other,
            bound_dir: None,
            in_use: false,
        };
        assert!(
            !shared.is_orphan(),
            "an unbound shared volume is never an orphan"
        );
    }

    /// The CLI names a language and version but never an arch, so a removal has
    /// to reach EVERY arch's copy plus the pre-arch location. Removing only the
    /// host's would leave the rest as space the user cannot see or reclaim.
    #[test]
    fn a_toolchain_target_covers_every_arch_and_the_legacy_path() {
        let targets = toolchain_target("dotnet", Some("9.0.308")).expect("valid");
        assert!(targets.contains(&"/t/arm64/dotnet/9.0.308".to_string()));
        assert!(targets.contains(&"/t/amd64/dotnet/9.0.308".to_string()));
        assert!(targets.contains(&"/t/dotnet/9.0.308".to_string()));
        assert_eq!(targets.len(), ARCH_SEGMENTS.len() + 1);

        let whole = toolchain_target("dotnet", None).expect("valid");
        assert!(whole.contains(&"/t/arm64/dotnet".to_string()));
        assert!(whole.contains(&"/t/dotnet".to_string()));
    }

    /// These segments are interpolated into a `rm -r` inside a `sh -c`, so an
    /// unsafe one is an escape out of the volume — `..` would walk to the volume
    /// root and a metacharacter would end the command and start another. Every
    /// case here must be rejected BEFORE the string is built.
    #[test]
    fn an_unsafe_segment_yields_no_target() {
        for (language, version) in [
            ("..", None),
            (".", None),
            ("", None),
            ("dotnet", Some("..")),
            ("dotnet", Some("../../etc")),
            ("dotnet/../..", None),
            ("dotnet;rm -rf /", None),
            ("dotnet $(id)", None),
            ("dotnet", Some("9.0.308; rm -r /t")),
        ] {
            assert_eq!(
                toolchain_target(language, version),
                None,
                "must reject {language:?} @ {version:?}"
            );
        }
        // The length cap is part of the guard, not decoration.
        assert_eq!(toolchain_target(&"a".repeat(65), None), None);
        assert!(toolchain_target(&"a".repeat(64), None).is_some());
    }
}
