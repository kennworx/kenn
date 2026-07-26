//! The shared toolchain cache and its atomic-install contract.
//!
//! Layout is `<root>/<arch>/<language>/<resolved-version>/`, keyed by the
//! CONCRETE version a pin resolved to — so two workspaces that declare the
//! toolchain differently but resolve identically share one installation.
//!
//! `<arch>` is part of the key because the cache is ONE volume shared by every
//! container on the host and a toolchain is a native binary. Without it a
//! mixed-arch host hands the second container the first one's toolchain, and
//! [`ToolchainCache::is_provisioned`] cannot tell: it is only "the destination
//! exists". Measured before the key existed — an amd64 container reported
//! `go version go1.26.5 linux/arm64`, building for the wrong target instead of
//! failing.
//!
//! # Why a present directory means a complete installation
//!
//! [`ToolchainCache::is_provisioned`] is just "the destination exists". That is
//! only sound because nothing ever writes into the destination incrementally:
//! an installer fills a staging directory, and the staging directory is
//! `rename`d into place in one atomic step. A run killed mid-unpack leaves
//! debris under `.staging/`, which no reader consults, and the destination
//! never appears at all.
//!
//! Installing directly into the destination would break that invariant
//! silently: a half-unpacked toolchain would look provisioned forever, and the
//! failure would surface much later as a broken index rather than a failed
//! install.

use std::path::{Path, PathBuf};

use fs2::FileExt;

/// Directory under the cache root holding in-progress installations. It shares
/// the root's filesystem, which `rename` requires. A language can never collide
/// with it: the leading dot is not a legal language name.
const STAGING_DIR: &str = ".staging";

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("toolchain cache: {context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// The installer callback failed. Its staging directory has been removed, so
    /// the cache is unchanged and the next run reprovisions from scratch.
    #[error("toolchain cache: installing {language} {version}: {reason}")]
    Install {
        language: String,
        version: String,
        reason: String,
    },
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CacheError {
    let context = context.into();
    move |source| CacheError::Io { context, source }
}

/// A machine-wide cache of provisioned language toolchains.
#[derive(Debug, Clone)]
pub struct ToolchainCache {
    root: PathBuf,
}

impl ToolchainCache {
    /// `arch` is the architecture segment (see [`Arch::cache_key`]), and it is
    /// folded into the root rather than into [`Self::path`] so that EVERY
    /// derived path — destination, lock file, staging dir — is arch-scoped by
    /// construction. A per-call arch parameter would leave each new call site
    /// free to forget it.
    ///
    /// [`Arch::cache_key`]: crate::resolve::Arch::cache_key
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, arch: &str) -> Self {
        Self {
            root: root.into().join(arch),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `language` at the resolved `version` lives, whether or not it is
    /// installed yet.
    #[must_use]
    pub fn path(&self, language: &str, version: &str) -> PathBuf {
        self.root.join(language).join(version)
    }

    /// Whether this toolchain is installed and complete. See the module docs for
    /// why directory existence is a sufficient test.
    #[must_use]
    pub fn is_provisioned(&self, language: &str, version: &str) -> bool {
        self.path(language, version).is_dir()
    }

    /// The provisioned versions of `language`, as their cache directory names.
    /// Only complete toolchains are returned: entries that are directories and
    /// not dot-prefixed — lock files (`.{version}.lock`) and in-flight staging
    /// live under dot names, and the atomic stage→rename means a visible non-dot
    /// dir is complete (same guarantee `is_provisioned` relies on). A cache with
    /// no directory for `language` yet lists none rather than erroring.
    #[must_use]
    pub fn provisioned_versions(&self, language: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.root.join(language)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect()
    }

    /// Ensure `language` at `version` is present, running `install` to populate a
    /// staging directory if it is not.
    ///
    /// `install` receives an empty directory and must fill it with the complete
    /// toolchain. It MUST NOT touch the returned destination path. On success the
    /// staging directory is renamed into place atomically; on failure it is
    /// removed and the cache is left untouched.
    ///
    /// Concurrent callers are serialized by an exclusive lock on the destination
    /// key, so exactly one downloads and the others observe the finished install.
    pub fn provision<F>(
        &self,
        language: &str,
        version: &str,
        install: F,
    ) -> Result<PathBuf, CacheError>
    where
        F: FnOnce(&Path) -> Result<(), String>,
    {
        let dest = self.path(language, version);
        // Fast path: already installed, no lock, no filesystem mutation.
        if dest.is_dir() {
            return Ok(dest);
        }

        let lang_dir = self.root.join(language);
        std::fs::create_dir_all(&lang_dir)
            .map_err(io_err(format!("creating {}", lang_dir.display())))?;

        let _guard = LockGuard::acquire(&lang_dir.join(format!(".{version}.lock")))?;

        // Re-check under the lock: another process may have finished while we
        // waited, in which case there is nothing to do.
        if dest.is_dir() {
            return Ok(dest);
        }

        let staging = self.fresh_staging_dir(language, version)?;
        if let Err(reason) = install(staging.path()) {
            return Err(CacheError::Install {
                language: language.to_string(),
                version: version.to_string(),
                reason,
            });
        }

        // The atomic step. Until this succeeds no reader can see a partial tree;
        // after it, the destination is complete by construction.
        match std::fs::rename(staging.path(), &dest) {
            Ok(()) => {
                staging.into_renamed();
                Ok(dest)
            }
            // A concurrent installer on another machine sharing this cache (or a
            // lock we could not take) may have populated the destination first.
            // Its content is as good as ours; keep theirs.
            Err(_) if dest.is_dir() => Ok(dest),
            Err(e) => Err(CacheError::Io {
                context: format!("installing {language} {version} into {}", dest.display()),
                source: e,
            }),
        }
    }

    /// A fresh, empty staging directory on the same filesystem as the cache root.
    /// The pid keeps concurrent processes apart; the counter keeps concurrent
    /// threads of one process apart.
    fn fresh_staging_dir(&self, language: &str, version: &str) -> Result<Staging, CacheError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);

        let staging_root = self.root.join(STAGING_DIR);
        std::fs::create_dir_all(&staging_root)
            .map_err(io_err(format!("creating {}", staging_root.display())))?;

        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = staging_root.join(format!("{language}-{version}-{}-{seq}", std::process::id()));
        // Debris from an earlier crash under the same pid+seq would otherwise be
        // handed to the installer as if it were empty.
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(io_err(format!("clearing stale staging {}", dir.display())))?;
        }
        std::fs::create_dir_all(&dir).map_err(io_err(format!("creating {}", dir.display())))?;
        Ok(Staging { dir: Some(dir) })
    }
}

/// Owns a staging directory and removes it unless it was renamed into place, so
/// a failed or panicking install cannot leave debris behind.
struct Staging {
    dir: Option<PathBuf>,
}

impl Staging {
    fn path(&self) -> &Path {
        self.dir.as_deref().unwrap_or(Path::new(""))
    }

    /// Give up ownership: the directory no longer exists under this name because
    /// it was renamed to its destination.
    fn into_renamed(mut self) {
        self.dir = None;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            // Best effort: debris under `.staging/` is never read, and the next
            // run allocates a fresh directory regardless.
            drop(std::fs::remove_dir_all(dir));
        }
    }
}

/// An exclusive `flock` on a per-destination lock file, released on drop.
struct LockGuard(std::fs::File);

impl LockGuard {
    fn acquire(path: &Path) -> Result<Self, CacheError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(io_err(format!("opening lock {}", path.display())))?;
        file.lock_exclusive()
            .map_err(io_err(format!("locking {}", path.display())))?;
        Ok(Self(file))
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Closing the file releases the flock anyway; this is belt and braces.
        drop(FileExt::unlock(&self.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (tempfile::TempDir, ToolchainCache) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = ToolchainCache::new(tmp.path(), "arm64");
        (tmp, cache)
    }

    #[test]
    fn path_is_language_then_resolved_version() {
        let (_tmp, c) = cache();
        assert_eq!(c.path("dotnet", "9.0.308"), c.root().join("dotnet/9.0.308"));
        assert!(!c.is_provisioned("dotnet", "9.0.308"));
    }

    #[test]
    fn provisioned_versions_lists_only_complete_version_dirs() {
        let (_tmp, c) = cache();
        let swift = c.root().join("swift");
        std::fs::create_dir_all(swift.join("6.0")).unwrap();
        std::fs::create_dir_all(swift.join("6.3")).unwrap();
        // A dot-prefixed staging dir and lock file, and a stray non-dir, are NOT
        // versions — they must be excluded, matching the host busybox glob.
        std::fs::create_dir_all(swift.join(".6.5.staging")).unwrap();
        std::fs::write(swift.join(".6.0.lock"), b"").unwrap();
        std::fs::write(swift.join("README"), b"").unwrap();

        let mut got = c.provisioned_versions("swift");
        got.sort();
        assert_eq!(got, vec!["6.0".to_string(), "6.3".to_string()]);

        // A language with no directory yet lists none, and does not error.
        assert!(c.provisioned_versions("go").is_empty());
    }

    /// The cache volume is shared by every container on the host, so two
    /// architectures must never resolve to one destination. They did: the key
    /// was `<language>/<version>` and `is_provisioned` is only "the destination
    /// exists", so an amd64 container found an arm64 install and used it —
    /// observed as `go version go1.26.5 linux/arm64` INSIDE an amd64 container,
    /// building for the wrong target instead of failing.
    #[test]
    fn two_architectures_do_not_share_a_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let arm = ToolchainCache::new(tmp.path(), "arm64");
        let amd = ToolchainCache::new(tmp.path(), "amd64");

        assert_ne!(arm.path("go", "1.26.5"), amd.path("go", "1.26.5"));

        arm.provision("go", "1.26.5", |staging| {
            std::fs::write(staging.join("go"), b"arm64").map_err(|e| e.to_string())
        })
        .expect("provision arm64");

        assert!(arm.is_provisioned("go", "1.26.5"));
        assert!(
            !amd.is_provisioned("go", "1.26.5"),
            "an arm64 install must not satisfy an amd64 lookup"
        );

        // And the amd64 installer must actually run rather than being skipped.
        let mut ran = false;
        amd.provision("go", "1.26.5", |staging| {
            ran = true;
            std::fs::write(staging.join("go"), b"amd64").map_err(|e| e.to_string())
        })
        .expect("provision amd64");
        assert!(ran, "amd64 installer must run despite the arm64 install");
    }

    #[test]
    fn a_successful_install_is_visible_and_reused() {
        let (_tmp, c) = cache();
        let dest = c
            .provision("dotnet", "9.0.308", |staging| {
                std::fs::write(staging.join("dotnet"), b"x").map_err(|e| e.to_string())
            })
            .expect("provision");

        assert!(dest.join("dotnet").is_file());
        assert!(c.is_provisioned("dotnet", "9.0.308"));

        // A second call must not run the installer again.
        c.provision("dotnet", "9.0.308", |_| {
            panic!("installer must not run for a provisioned toolchain")
        })
        .expect("reuse");
    }

    /// The invariant the whole design rests on: a run that dies part-way through
    /// unpacking must leave NOTHING a later run can mistake for a complete
    /// toolchain.
    ///
    /// Mutation-checked: installing into the destination instead of staging
    /// makes this fail — `is_provisioned` returns true for the half-written tree
    /// and the retry's installer never runs.
    #[test]
    fn an_interrupted_install_leaves_nothing_usable() {
        let (_tmp, c) = cache();

        let err = c
            .provision("dotnet", "9.0.308", |staging| {
                // Partial unpack: some files land, then the install dies.
                std::fs::write(staging.join("partial"), b"half a toolchain")
                    .map_err(|e| e.to_string())?;
                Err("interrupted mid-unpack".to_string())
            })
            .expect_err("a failed install must be an error");
        assert!(matches!(err, CacheError::Install { .. }), "{err}");

        assert!(
            !c.is_provisioned("dotnet", "9.0.308"),
            "a half-unpacked toolchain must never look provisioned"
        );

        // And the retry must actually reprovision, not short-circuit.
        let ran = std::cell::Cell::new(false);
        let dest = c
            .provision("dotnet", "9.0.308", |staging| {
                ran.set(true);
                std::fs::write(staging.join("dotnet"), b"x").map_err(|e| e.to_string())
            })
            .expect("retry provisions");
        assert!(ran.get(), "the retry must run the installer");
        assert!(dest.join("dotnet").is_file());
        assert!(!dest.join("partial").exists(), "debris must not survive");
    }

    #[test]
    fn the_installer_receives_an_empty_directory() {
        let (_tmp, c) = cache();
        c.provision("go", "1.24.0", |staging| {
            let entries = std::fs::read_dir(staging)
                .map_err(|e| e.to_string())?
                .count();
            if entries == 0 {
                Ok(())
            } else {
                Err(format!("staging had {entries} pre-existing entries"))
            }
        })
        .expect("provision");
    }

    #[test]
    fn distinct_versions_do_not_share_a_directory() {
        let (_tmp, c) = cache();
        for v in ["8.0.404", "9.0.308"] {
            c.provision("dotnet", v, |staging| {
                std::fs::write(staging.join("version"), v).map_err(|e| e.to_string())
            })
            .expect("provision");
        }
        assert_eq!(
            std::fs::read_to_string(c.path("dotnet", "8.0.404").join("version")).unwrap(),
            "8.0.404"
        );
        assert_eq!(
            std::fs::read_to_string(c.path("dotnet", "9.0.308").join("version")).unwrap(),
            "9.0.308"
        );
    }

    #[test]
    fn concurrent_provisioners_install_once_and_agree() {
        let (_tmp, c) = cache();
        let installs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        std::thread::scope(|s| {
            for _ in 0..8 {
                let c = c.clone();
                let installs = std::sync::Arc::clone(&installs);
                s.spawn(move || {
                    c.provision("dotnet", "9.0.308", |staging| {
                        installs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        // Widen the window a real download would leave open.
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        std::fs::write(staging.join("dotnet"), b"x").map_err(|e| e.to_string())
                    })
                    .expect("provision");
                });
            }
        });

        assert_eq!(
            installs.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the lock must let exactly one installer run"
        );
        assert!(c.is_provisioned("dotnet", "9.0.308"));
    }

    #[test]
    fn staging_debris_does_not_count_as_provisioned() {
        let (_tmp, c) = cache();
        drop(c.provision("dotnet", "9.0.308", |staging| {
            std::fs::write(staging.join("partial"), b"x").map_err(|e| e.to_string())?;
            Err("boom".to_string())
        }));
        // Whatever is left under .staging, the language dir must hold no version.
        assert!(!c.is_provisioned("dotnet", "9.0.308"));
    }
}
