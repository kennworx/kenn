//! `Layout` — the resolved on-disk shape for one workspace's store plus
//! every path accessor against it. `StoreError` lives here too because
//! its construction sites are spread across this and sibling modules.

use std::fs;
use std::path::{Path, PathBuf};

use kenn_config::Config;
use thiserror::Error;

use super::resolve::{resolve_location_spec, same_filesystem, RUN_ID_FORMAT};

/// The anchor directory for the vectors root: the repo's main worktree,
/// falling back to `source_root` outside a git tree. When `source_root`
/// *is* the main worktree, the caller's own (possibly non-canonical)
/// spelling is preserved so default layouts compare equal to the
/// pre-shared-cache paths.
fn vectors_anchor(source_root: &Path) -> PathBuf {
    let Some(main) = crate::git::main_worktree(source_root) else {
        return source_root.to_path_buf();
    };
    let canonical_src = fs::canonicalize(source_root).unwrap_or_else(|_| source_root.to_path_buf());
    if main == canonical_src {
        source_root.to_path_buf()
    } else {
        main
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("`{0}` is not a directory")]
    NotADirectory(PathBuf),
    #[error("incompatible configuration: {0}")]
    Config(String),
}

/// The resolved on-disk layout for one workspace's store. Cheap to clone.
///
/// Resolved once via [`Layout::resolve`] (config-driven) or
/// [`Layout::default_for`] (the in-repo default), then threaded through
/// every store-touching component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub(super) source_root: PathBuf,
    pub(super) committed_root: PathBuf,
    pub(super) derived_root: PathBuf,
    pub(super) vectors_root: PathBuf,
    /// `true` when `derived_root` and `vectors_root` share a filesystem
    /// (the common case — both default to subdirectories of
    /// `<committed_root>`). When false, sidecar tmp scratch falls back
    /// to a `.tmp` directory co-located with vectors so that
    /// atomic-rename does not return `EXDEV`. Computed once at resolve
    /// time by comparing device ids of the nearest existing ancestor
    /// of each root (per design D8).
    pub(super) vectors_share_fs_with_derived: bool,
    /// `[vectors] cache_cap_mb` — the multi-generation cache's GC size
    /// cap, carried on the layout so the embed pass can trigger GC
    /// without re-loading config. `0` disables GC.
    pub(super) vectors_cache_cap_mb: u64,
}

impl Layout {
    /// The in-repo default layout for `source_root`: committed data under
    /// `<source_root>/.kenn`, derived data under `<source_root>/.kenn/local`,
    /// vectors under the **main worktree's** `.kenn/vectors`
    /// (`shared-vector-cache` Phase 3) — for the main worktree or a non-git
    /// directory that is `<source_root>/.kenn/vectors`; for a linked
    /// worktree it is the main tree's shared dir, so worktrees reuse each
    /// other's vectors out of the box.
    #[must_use]
    pub fn default_for(source_root: &Path) -> Self {
        let committed_root = source_root.join(".kenn");
        let derived_root = committed_root.join("local");
        let anchor = vectors_anchor(source_root);
        let vectors_root = anchor.join(".kenn").join("vectors");
        let vectors_share_fs_with_derived =
            anchor == source_root || same_filesystem(&derived_root, &vectors_root);
        Self {
            source_root: source_root.to_path_buf(),
            committed_root,
            derived_root,
            vectors_root,
            vectors_share_fs_with_derived,
            vectors_cache_cap_mb: kenn_config::VectorsConfig::default().cache_cap_mb,
        }
    }

    /// Resolve the layout from configuration.
    ///
    /// `source_root` is the fallback source root; `[workspace] root`
    /// overrides it when set. `committed_root` is always
    /// `<source_root>/.kenn`. `derived_root` comes from
    /// `[layout] derived_root` and `vectors_root` from
    /// `[vectors] location` — each accepts a relative path (resolved
    /// from the source root), an absolute path, or the keyword
    /// `"global"` (an XDG-cache path keyed by a per-repository project
    /// id). `derived_root` defaults to `<committed_root>/local`;
    /// `vectors_root` defaults to `<committed_root>/vectors`.
    ///
    /// A `derived_root` set away from the in-repo default is shared
    /// across the repository's branches, so it requires
    /// `staleness.git_aware_skip = true` to keep them apart by staleness
    /// key; resolution fails otherwise.
    pub fn resolve(config: &Config, source_root: &Path) -> Result<Self, StoreError> {
        let source_root = config
            .workspace
            .root
            .clone()
            .unwrap_or_else(|| source_root.to_path_buf());
        let committed_root = source_root.join(".kenn");
        let default_derived = committed_root.join("local");
        // Vectors anchor at the repo's main worktree (`shared-vector-cache`):
        // the default and any relative `[vectors] location` resolve there, so
        // every linked worktree shares one content-addressed vector cache.
        // The derived store stays per-worktree (anchored at the source root).
        let anchor = vectors_anchor(&source_root);
        let default_vectors = anchor.join(".kenn").join("vectors");

        let derived_root = resolve_location_spec(
            config.layout.derived_root.as_deref(),
            &source_root,
            &source_root,
            "kenn",
            &default_derived,
        )?;
        let vectors_root = resolve_location_spec(
            config.vectors.location.as_deref(),
            &source_root,
            &anchor,
            "kenn-vectors",
            &default_vectors,
        )?;

        if derived_root != default_derived && !config.staleness.git_aware_skip {
            return Err(StoreError::Config(
                "`[layout] derived_root` is relocated away from the in-repo default, \
                 but `[staleness] git_aware_skip` is false — a shared derived root \
                 relies on staleness keys to keep branches apart. Set \
                 `staleness.git_aware_skip = true` or remove `layout.derived_root`."
                    .to_owned(),
            ));
        }

        let vectors_share_fs_with_derived = same_filesystem(&derived_root, &vectors_root);

        Ok(Self {
            source_root,
            committed_root,
            derived_root,
            vectors_root,
            vectors_share_fs_with_derived,
            vectors_cache_cap_mb: config.vectors.cache_cap_mb,
        })
    }

    #[must_use]
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// The committed, git-tracked root — always `<source_root>/.kenn`.
    #[must_use]
    pub fn committed_root(&self) -> &Path {
        &self.committed_root
    }

    /// The derived, gitignored, relocatable root.
    #[must_use]
    pub fn derived_root(&self) -> &Path {
        &self.derived_root
    }

    /// The committed vectors root — the parent of every generation
    /// directory and the legacy flat `code/`/`findings/` dirs. Defaults
    /// to the **main worktree's** `.kenn/vectors` (shared across linked
    /// worktrees); relocatable via `[vectors] location`.
    #[must_use]
    pub fn vectors_root(&self) -> &Path {
        &self.vectors_root
    }

    /// `[vectors] cache_cap_mb` — the GC size cap for the multi-
    /// generation vector cache. `0` disables GC.
    #[must_use]
    pub fn vectors_cache_cap_mb(&self) -> u64 {
        self.vectors_cache_cap_mb
    }

    /// Test-only override of the resolved vectors root, so sidecar tests
    /// can point a layout at a fixture directory.
    #[cfg(test)]
    pub(crate) fn set_vectors_root_for_tests(&mut self, root: PathBuf) {
        self.vectors_root = root;
    }

    // ── committed artifacts ─────────────────────────────────────────

    /// The **legacy** flat code sidecar — `<vectors_root>/code/`, the
    /// pre-generation location. Still read as a reuse fallback (so
    /// committed `pack-*.bin` files keep serving fresh clones) and
    /// enumerated for repack/GC; new writes go to the generation dir
    /// (`code_generation_dir`).
    #[must_use]
    pub fn code_vectors_dir(&self) -> PathBuf {
        self.vectors_root.join("code")
    }

    /// The committed per-finding records — `.kenn/findings/`.
    #[must_use]
    pub fn findings_dir(&self) -> PathBuf {
        self.committed_root.join("findings")
    }

    /// The **legacy** flat findings sidecar — `<vectors_root>/findings/`
    /// (see [`Self::code_vectors_dir`]). New writes go to the generation
    /// dir (`findings_generation_dir`).
    #[must_use]
    pub fn findings_vectors_dir(&self) -> PathBuf {
        self.vectors_root.join("findings")
    }

    /// The committed `.gitignore`.
    #[must_use]
    pub fn gitignore_path(&self) -> PathBuf {
        self.committed_root.join(".gitignore")
    }

    /// Legacy `<derived_root>/findings/` swept at startup so a prior
    /// layout's directory doesn't linger.
    #[must_use]
    pub fn legacy_findings_dir(&self) -> PathBuf {
        self.derived_root.join("findings")
    }

    /// `<derived_root>/live` — a small text pointer file naming the active
    /// run by a path relative to this directory (D1).
    #[must_use]
    pub fn live_path(&self) -> PathBuf {
        self.derived_root.join("live")
    }

    /// Resolve `live` to its target run directory. The single `live` reader
    /// in the workspace (`Store::live_target` delegates here, D2).
    ///
    /// Degrades to `None` — never panics — when `live` is absent, empty, or
    /// (on an old store) still a symlink: `read_to_string` follows a symlink
    /// to a directory and errors, so the user sees a reindex, per the
    /// no-migration policy (D3).
    #[must_use]
    pub fn live_target(&self) -> Option<PathBuf> {
        let contents = fs::read_to_string(self.live_path()).ok()?;
        let target = contents.trim();
        if target.is_empty() {
            return None;
        }
        let target = Path::new(target);
        let resolved = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.derived_root.join(target)
        };
        resolved.is_dir().then_some(resolved)
    }

    /// `<derived_root>/runs/` — parent of per-pass run dirs.
    #[must_use]
    pub fn runs_dir(&self) -> PathBuf {
        self.derived_root.join("runs")
    }

    /// `<derived_root>/runs/{id}/` — one index pass's working directory.
    #[must_use]
    pub fn run_dir(&self, id: &str) -> PathBuf {
        self.runs_dir().join(id)
    }

    /// `<derived_root>/runs/{id}/{lang}.scip`.
    #[must_use]
    pub fn run_scip_path(&self, id: &str, lang: &str) -> PathBuf {
        self.run_dir(id).join(format!("{lang}.scip"))
    }

    /// `<derived_root>/runs/{id}/{lang}.jsonl`.
    #[must_use]
    pub fn run_jsonl_path(&self, id: &str, lang: &str) -> PathBuf {
        self.run_dir(id).join(format!("{lang}.jsonl"))
    }

    /// `<derived_root>/runs/{id}/atlas/` — the run's OKF atlas bundle
    /// (`atlas` capability). Written during finalize and carried on publish, so
    /// the published bundle lands at `<snapshot>/atlas/`. Lives under the
    /// derived (gitignored) root: the atlas is a regenerated build artifact.
    #[must_use]
    pub fn run_atlas_dir(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("atlas")
    }

    /// `<derived_root>/runs/{id}/tmp/` — atomic-rename scratch.
    #[must_use]
    pub fn run_tmp_dir(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("tmp")
    }

    /// Scratch directory the sidecar writer should use for tmp files
    /// that will be renamed into `<vectors_root>/{code|findings}/`.
    #[must_use]
    pub fn writer_tmp_dir(&self, run_id: &str) -> PathBuf {
        if self.vectors_share_fs_with_derived {
            self.run_tmp_dir(run_id)
        } else {
            self.vectors_root.join(".tmp")
        }
    }

    /// `true` when the writer tmp dir lives under the active run directory.
    #[must_use]
    pub fn vectors_share_fs_with_derived(&self) -> bool {
        self.vectors_share_fs_with_derived
    }

    /// Scratch directory for sidecar writes that aren't bound to a
    /// specific indexer run.
    #[must_use]
    pub fn sidecar_tmp_dir(&self) -> PathBuf {
        if self.vectors_share_fs_with_derived {
            self.derived_root.join("embed-tmp")
        } else {
            self.vectors_root.join(".tmp")
        }
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.derived_root.join("index.lock")
    }

    /// `scip-<slug>.scip` indexer intermediate — derived artifact.
    /// Deprecated location; new shape (D4) puts SCIP at `run_scip_path`.
    #[must_use]
    pub fn scip_path(&self, slug: &str) -> PathBuf {
        self.derived_root.join(format!("scip-{slug}.scip"))
    }

    /// Cross-process reader registry — `<derived_root>/readers/`.
    #[must_use]
    pub fn readers_dir(&self) -> PathBuf {
        self.derived_root.join("readers")
    }

    /// Workspace-wide flock fencing `store_finding` against indexer flip.
    #[must_use]
    pub fn findings_publish_lock_path(&self) -> PathBuf {
        self.derived_root.join("findings-publish.lock")
    }

    /// Emit a fresh run id in `YYYY-MM-DDTHH-MM-SSZ` format. If
    /// `last_id` was emitted in the same wall-clock second, appends
    /// `-1`, `-2`, … to disambiguate.
    pub fn new_run_id(
        now: time::OffsetDateTime,
        last_id: Option<&str>,
    ) -> Result<String, StoreError> {
        let base = now
            .format(RUN_ID_FORMAT)
            .map_err(|e| StoreError::Config(format!("formatting run id timestamp: {e}")))?;
        let Some(last) = last_id else {
            return Ok(base);
        };
        if !last.starts_with(&base) {
            return Ok(base);
        }
        let suffix_idx = last
            .strip_prefix(&base)
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.parse::<u32>().ok())
            .map_or(1, |n| n + 1);
        Ok(format!("{base}-{suffix_idx}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        TempDir::new().unwrap()
    }

    fn config(toml: &str) -> Config {
        Config::from_toml(toml).unwrap()
    }

    #[test]
    fn default_layout_matches_in_repo_paths() {
        let ws = workspace();
        let layout = Layout::resolve(&config(""), ws.path()).unwrap();
        assert_eq!(layout.committed_root(), ws.path().join(".kenn"));
        assert_eq!(layout.derived_root(), ws.path().join(".kenn").join("local"));
        assert_eq!(layout, Layout::default_for(ws.path()));
    }

    #[test]
    fn default_layout_accessors() {
        let ws = workspace();
        let l = Layout::default_for(ws.path());
        let kenn = ws.path().join(".kenn");
        let local = kenn.join("local");
        let vectors = kenn.join("vectors");
        assert_eq!(l.vectors_root(), vectors);
        assert_eq!(l.code_vectors_dir(), vectors.join("code"));
        assert_eq!(l.findings_dir(), kenn.join("findings"));
        assert_eq!(l.findings_vectors_dir(), vectors.join("findings"));
        assert_eq!(l.live_path(), local.join("live"));
        assert_eq!(l.runs_dir(), local.join("runs"));
        assert_eq!(l.lock_path(), local.join("index.lock"));
        assert_eq!(l.scip_path("rust"), local.join("scip-rust.scip"));
        let run = local.join("runs").join("2026-05-01T15-30-00Z");
        assert_eq!(l.run_dir("2026-05-01T15-30-00Z"), run);
        assert_eq!(
            l.run_scip_path("2026-05-01T15-30-00Z", "rust"),
            run.join("rust.scip")
        );
        assert_eq!(
            l.run_jsonl_path("2026-05-01T15-30-00Z", "rust"),
            run.join("rust.jsonl")
        );
        assert_eq!(l.run_tmp_dir("2026-05-01T15-30-00Z"), run.join("tmp"));
        assert!(l.vectors_share_fs_with_derived());
        assert_eq!(l.writer_tmp_dir("2026-05-01T15-30-00Z"), run.join("tmp"));
    }

    #[test]
    fn configured_derived_root_is_honored() {
        let ws = workspace();
        let other = workspace();
        let toml = format!(
            "[layout]\nderived_root = {:?}\n",
            other.path().to_str().unwrap()
        );
        let l = Layout::resolve(&config(&toml), ws.path()).unwrap();
        assert_eq!(l.committed_root(), ws.path().join(".kenn"));
        assert_eq!(l.derived_root(), other.path());
        assert_eq!(l.runs_dir(), other.path().join("runs"));
        assert_eq!(l.scip_path("rust"), other.path().join("scip-rust.scip"));
        assert_eq!(l.vectors_root(), ws.path().join(".kenn").join("vectors"));
        assert_eq!(
            l.code_vectors_dir(),
            ws.path().join(".kenn").join("vectors").join("code")
        );
    }

    #[test]
    fn vectors_location_override_relative() {
        let ws = workspace();
        let l = Layout::resolve(
            &config("[vectors]\nlocation = \"team-vectors\"\n"),
            ws.path(),
        )
        .unwrap();
        let expected = ws.path().join("team-vectors");
        assert_eq!(l.vectors_root(), expected);
        assert_eq!(l.code_vectors_dir(), expected.join("code"));
        assert_eq!(l.findings_vectors_dir(), expected.join("findings"));
        assert_eq!(l.derived_root(), ws.path().join(".kenn").join("local"));
    }

    #[test]
    fn vectors_location_override_absolute() {
        let ws = workspace();
        let other = workspace();
        let toml = format!(
            "[vectors]\nlocation = {:?}\n",
            other.path().to_str().unwrap()
        );
        let l = Layout::resolve(&config(&toml), ws.path()).unwrap();
        assert_eq!(l.vectors_root(), other.path());
        assert_eq!(l.code_vectors_dir(), other.path().join("code"));
        assert_eq!(l.findings_vectors_dir(), other.path().join("findings"));
    }

    #[test]
    fn vectors_location_override_global() {
        let ws = workspace();
        let l = Layout::resolve(&config("[vectors]\nlocation = \"global\"\n"), ws.path()).unwrap();
        assert!(
            l.vectors_root().starts_with(
                std::env::var_os("XDG_CACHE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                    .unwrap()
                    .join("kenn-vectors")
            ),
            "got {:?}",
            l.vectors_root()
        );
        let other = workspace();
        let l2 =
            Layout::resolve(&config("[vectors]\nlocation = \"global\"\n"), other.path()).unwrap();
        assert_ne!(l.vectors_root(), l2.vectors_root());
    }

    #[test]
    fn writer_tmp_dir_falls_back_when_vectors_on_other_fs() {
        let ws = workspace();
        let mut l = Layout::default_for(ws.path());
        l.vectors_share_fs_with_derived = false;
        l.vectors_root = PathBuf::from("/mnt/shared/vectors");
        assert!(!l.vectors_share_fs_with_derived());
        assert_eq!(
            l.writer_tmp_dir("2026-05-01T15-30-00Z"),
            PathBuf::from("/mnt/shared/vectors/.tmp")
        );
    }

    #[test]
    fn new_run_id_formats_timestamp_and_disambiguates_collisions() {
        use time::macros::datetime;
        let t = datetime!(2026-05-24 10:35:34 UTC);
        assert_eq!(Layout::new_run_id(t, None).unwrap(), "2026-05-24T10-35-34Z");
        assert_eq!(
            Layout::new_run_id(t, Some("2026-05-24T10-35-34Z")).unwrap(),
            "2026-05-24T10-35-34Z-1"
        );
        assert_eq!(
            Layout::new_run_id(t, Some("2026-05-24T10-35-34Z-1")).unwrap(),
            "2026-05-24T10-35-34Z-2"
        );
        assert_eq!(
            Layout::new_run_id(t, Some("2026-05-24T10-35-33Z")).unwrap(),
            "2026-05-24T10-35-34Z"
        );
    }

    #[test]
    fn vectors_and_derived_global_do_not_collide() {
        let ws = workspace();
        let l = Layout::resolve(
            &config(
                "[layout]\nderived_root = \"global\"\n\
                 [vectors]\nlocation = \"global\"\n\
                 [staleness]\ngit_aware_skip = true\n",
            ),
            ws.path(),
        )
        .unwrap();
        assert_ne!(l.vectors_root(), l.derived_root());
    }

    #[test]
    fn relative_derived_root_resolves_from_source_root() {
        let ws = workspace();
        let l = Layout::resolve(
            &config("[layout]\nderived_root = \"build/idx\"\n"),
            ws.path(),
        )
        .unwrap();
        assert_eq!(l.derived_root(), ws.path().join("build").join("idx"));
    }

    #[test]
    fn global_derived_root_is_unique_and_in_cache() {
        let ws = workspace();
        let l =
            Layout::resolve(&config("[layout]\nderived_root = \"global\"\n"), ws.path()).unwrap();
        assert!(l.derived_root().starts_with(
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                .unwrap()
                .join("kenn")
        ));
        assert_eq!(l.committed_root(), ws.path().join(".kenn"));
        let other = workspace();
        let l2 = Layout::resolve(
            &config("[layout]\nderived_root = \"global\"\n"),
            other.path(),
        )
        .unwrap();
        assert_ne!(l.derived_root(), l2.derived_root());
    }

    #[test]
    fn relocated_derived_root_rejects_git_aware_skip_off() {
        let ws = workspace();
        let toml = "[layout]\nderived_root = \"/var/cache/kenn\"\n\
                    [staleness]\ngit_aware_skip = false\n";
        let err = Layout::resolve(&config(toml), ws.path()).unwrap_err();
        assert!(matches!(err, StoreError::Config(_)), "got {err:?}");
    }

    #[test]
    fn default_derived_root_allows_git_aware_skip_off() {
        let ws = workspace();
        Layout::resolve(&config("[staleness]\ngit_aware_skip = false\n"), ws.path())
            .expect("default layout resolves regardless of git_aware_skip");
    }
}
