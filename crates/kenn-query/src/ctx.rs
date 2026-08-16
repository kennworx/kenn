//! [`QueryCtx`] — the argument every read query takes, the caches a host lends
//! it, and the findings-store guards that hang off it.

use crate::cursor::SnapshotId;
use crate::error::{QueryError, QueryErrorCode};
use crate::result_cache::ResultCache;
use crate::types::{EmbedStage, RankedFindingView, SearchHitRef};

/// Everything a read query needs, and nothing a daemon knows.
///
/// This is the argument every query takes instead of a server state. The
/// difference is the whole point of the split: the MCP server's state carries a
/// lifecycle, a file watcher, and an MCP peer, none of which is a fact about the
/// code being queried. A query that cannot observe them cannot depend on them,
/// and can be exercised from a test that never starts a server.
///
/// Borrowed, not owned. The snapshot pin lives in the view the caller holds for
/// the duration of the call, so the context is a cheap view over it plus the
/// handful of things the host contributes — measured, not guessed: `source_root`
/// (6 sites), the result caches, `embed_stage`, the config pair, and the
/// findings store (24). Notably absent because nothing reads them: the embed
/// error, the staleness flag, the store layout.
///
/// `embed_stage` is a snapshotted value rather than a shared cell. Queries only
/// ever read it, to tell "still building" from "genuinely missing", and a value
/// taken once per call is the honest reading — re-loading mid-query could report
/// two different stages for one answer.
pub struct QueryCtx<'a> {
    /// The pooled connection for this call.
    pub read: &'a kenn_store::DbConn,
    /// When the snapshot was built — reported by `get_workspace_overview`.
    /// The §1.3 inventory recorded this as having no readers; that was wrong,
    /// and the compiler said so the moment its one reader migrated.
    pub indexed_at: &'a str,
    pub snapshot_id: SnapshotId,
    /// Workspace root, for resolving anchors and finding-relative paths.
    pub source_root: std::path::PathBuf,
    pub config: &'a kenn_config::Config,
    /// Whether a `kenn.toml` exists — distinguishes a never-initialized
    /// workspace from one whose config enables no language.
    pub config_present: bool,
    pub embed_stage: EmbedStage,
    pub findings: &'a tokio::sync::RwLock<Option<kenn_store::FindingsStore>>,
    /// The host's top-K page caches, borrowed for this call.
    pub caches: &'a QueryCaches,
}

/// The top-K result caches a host owns on behalf of its queries.
///
/// They outlive any single query — a cursor issued by one call is redeemed by
/// the next — so the host holds them and lends them to each [`QueryCtx`]. Held
/// together in one struct because they share one lifetime rule: both are
/// cleared when the snapshot rotates, since a cursor into a page of the old
/// snapshot must not silently resolve against the new one.
#[derive(Default)]
pub struct QueryCaches {
    /// Materialized top-K `search_symbols` pages, keyed by the `cache_id` that
    /// rides in `TopK` cursors.
    pub symbols: ResultCache<SearchHitRef>,
    /// Materialized top-K `search_findings` pages. Mirrors the symbol cache.
    pub findings: ResultCache<RankedFindingView>,
}

impl QueryCaches {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every cached page. Called on snapshot rotation, so a `TopK` cursor
    /// issued against the previous snapshot misses and reports `STALE_CURSOR`
    /// rather than paging into a result set that no longer describes the code.
    pub fn clear(&self) {
        self.symbols.clear();
        self.findings.clear();
    }
}

/// The open findings store under a shared read lock.
///
/// Exists so a query can hold the lock in its OWN scope. The `with_findings_*`
/// closures took a snapshot view by value while holding this guard, which is
/// what made them impossible to hand a borrowed [`QueryCtx`] — the guard's
/// lifetime and the context's borrow could not both be satisfied through one
/// closure signature. Returning the guard instead moves that problem to the
/// caller, where it is not a problem at all: both simply live in the same scope.
///
/// It also deletes the `Box::pin(async move { … })` every call site needed to
/// satisfy the closure's boxed-future bound.
pub struct FindingsRead<'a>(tokio::sync::RwLockReadGuard<'a, Option<kenn_store::FindingsStore>>);

impl std::ops::Deref for FindingsRead<'_> {
    type Target = kenn_store::FindingsStore;
    fn deref(&self) -> &Self::Target {
        // Checked at construction: `findings_read` returns an error rather
        // than a guard when the store failed to open.
        self.0.as_ref().expect("findings store present")
    }
}

/// The open findings store under the exclusive write lock. Only the mutating
/// queries (`store_finding`, `merge_findings`, `record_anchor`) take this.
pub struct FindingsWrite<'a>(tokio::sync::RwLockWriteGuard<'a, Option<kenn_store::FindingsStore>>);

impl std::ops::Deref for FindingsWrite<'_> {
    type Target = kenn_store::FindingsStore;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("findings store present")
    }
}

impl std::ops::DerefMut for FindingsWrite<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("findings store present")
    }
}

impl QueryCtx<'_> {
    /// Open the findings store for reading.
    ///
    /// # Errors
    /// `INTERNAL_ERROR` when the store failed to open at startup.
    pub async fn findings_read(&self) -> Result<FindingsRead<'_>, QueryError> {
        let guard = self.findings.read().await;
        if guard.is_none() {
            return Err(QueryError::new(
                QueryErrorCode::InternalError,
                "findings store unavailable — failed to open at startup",
            ));
        }
        Ok(FindingsRead(guard))
    }

    /// Open the findings store for writing.
    ///
    /// # Errors
    /// `INTERNAL_ERROR` when the store failed to open at startup.
    pub async fn findings_write(&self) -> Result<FindingsWrite<'_>, QueryError> {
        let guard = self.findings.write().await;
        if guard.is_none() {
            return Err(QueryError::new(
                QueryErrorCode::InternalError,
                "findings store unavailable — failed to open at startup",
            ));
        }
        Ok(FindingsWrite(guard))
    }
}
