//! `Store` — an opened on-disk handle over a resolved [`Layout`].
//! `Store::open` creates `derived_root` + the committed `.gitignore`
//! and exposes the lifecycle paths. `RunMeta` is the per-run summary
//! returned by [`Store::list_runs`].

use std::fs;
use std::path::{Path, PathBuf};

use super::gitignore::write_gitignore;
use super::types::{Layout, StoreError};

/// Metadata about one published run — its id (the directory leaf
/// name, an ISO-8601 timestamp) and absolute path. Returned by
/// [`Store::list_runs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    pub id: String,
    pub path: PathBuf,
}

/// On-disk handle for one workspace's index store, over a resolved
/// [`Layout`]. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    layout: Layout,
}

impl Store {
    /// Open a store with the in-repo default layout for `source_root`.
    pub fn open_default(source_root: &Path) -> Result<Self, StoreError> {
        Self::open(Layout::default_for(source_root))
    }

    /// Open a store over a resolved [`Layout`], creating `derived_root`
    /// and the committed `.gitignore` if absent. Idempotent.
    pub fn open(layout: Layout) -> Result<Self, StoreError> {
        if !layout.source_root().is_dir() {
            return Err(StoreError::NotADirectory(
                layout.source_root().to_path_buf(),
            ));
        }
        fs::create_dir_all(layout.derived_root())?;
        write_gitignore(&layout)?;
        Ok(Self { layout })
    }

    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The committed root — `<source_root>/.kenn`.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.layout.committed_root()
    }

    /// The gitignored, relocatable derived subtree.
    #[must_use]
    pub fn local_dir(&self) -> PathBuf {
        self.layout.derived_root().to_path_buf()
    }

    #[must_use]
    pub fn live_path(&self) -> PathBuf {
        self.layout.live_path()
    }

    #[must_use]
    pub fn runs_dir(&self) -> PathBuf {
        self.layout.runs_dir()
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.layout.lock_path()
    }

    #[must_use]
    pub fn readers_dir(&self) -> PathBuf {
        self.layout.readers_dir()
    }

    #[must_use]
    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.layout.run_dir(run_id)
    }

    #[must_use]
    pub fn report_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("report.json")
    }

    /// Resolve `live` to its target snapshot directory. Delegates to the
    /// single reader on [`Layout`] so the pointer-file format is read in
    /// exactly one place (D2).
    #[must_use]
    pub fn live_target(&self) -> Option<PathBuf> {
        self.layout.live_target()
    }

    /// Run directories under `runs/`, sorted ascending by id.
    pub fn list_runs(&self) -> Result<Vec<RunMeta>, StoreError> {
        let dir = self.runs_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(RunMeta {
                        id: name.to_string(),
                        path: entry.path(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn open_creates_kenn_dir() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        assert!(store.root().is_dir());
        assert_eq!(store.root().file_name().unwrap(), ".kenn");
    }

    #[test]
    fn first_time_init_has_no_live() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        assert!(!store.live_path().exists());
        assert!(store.list_runs().unwrap().is_empty());
        assert!(store.live_target().is_none());
    }

    #[test]
    fn steady_state_layout_after_one_run() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        let run_dir = store.runs_dir().join("2026-05-01T15-30-00Z");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(store.live_path(), "runs/2026-05-01T15-30-00Z").unwrap();

        assert!(store.live_path().is_file());
        assert!(!store.live_path().is_symlink());
        assert_eq!(store.live_target().unwrap(), run_dir);
        let runs = store.list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "2026-05-01T15-30-00Z");
    }

    #[cfg(unix)]
    #[test]
    fn old_symlink_live_degrades_to_none() {
        // D3: a store upgraded from a pre-pointer-file binary has `live` as a
        // symlink into the run dir. The pointer-file reader must resolve it to
        // None (→ reindex), not panic and not follow the symlink.
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        let run_dir = store.runs_dir().join("2026-05-01T15-30-00Z");
        fs::create_dir_all(&run_dir).unwrap();
        std::os::unix::fs::symlink("runs/2026-05-01T15-30-00Z", store.live_path()).unwrap();
        assert!(store.live_path().is_symlink());
        assert!(
            store.live_target().is_none(),
            "an old-store `live` symlink must degrade to None, not be followed"
        );
    }

    #[test]
    fn list_runs_sorted_ascending() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        for ts in [
            "2026-05-01T15-30-00Z",
            "2026-05-02T10-00-00Z",
            "2026-05-01T08-00-00Z",
        ] {
            fs::create_dir_all(store.runs_dir().join(ts)).unwrap();
        }
        let runs: Vec<_> = store
            .list_runs()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            runs,
            vec![
                "2026-05-01T08-00-00Z".to_string(),
                "2026-05-01T15-30-00Z".to_string(),
                "2026-05-02T10-00-00Z".to_string(),
            ]
        );
    }

    #[test]
    fn run_dir_and_report_path() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        let runs = store.local_dir().join("runs");
        assert_eq!(store.run_dir("run-42"), runs.join("run-42"));
        assert_eq!(
            store.report_path("run-42"),
            runs.join("run-42").join("report.json")
        );
    }

    #[test]
    fn local_dir_holds_the_derived_subtree() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        assert_eq!(store.local_dir(), store.root().join("local"));
        assert!(store.runs_dir().starts_with(store.local_dir()));
        assert!(store.lock_path().starts_with(store.local_dir()));
    }

    #[test]
    fn open_rejects_missing_workspace() {
        let err = Store::open_default(Path::new("/nonexistent/xyz/abc")).unwrap_err();
        assert!(matches!(err, StoreError::NotADirectory(_)));
    }
}
