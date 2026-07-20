//! `WriteBatch` — the value type the active backend's `write_batch`
//! operation consumes.
//!
//! There is no `Writer` trait: the backend is selected at compile time,
//! so the indexer's DB-writer thread calls the concrete writer's public
//! inherent operations directly. Record batching is done inline by that
//! thread as it drains the ingester record channel.
//!
//! Failure semantics are intentionally simple: if a batch write fails
//! mid-run, the caller drops the writer and re-ingests from the source
//! corpus. A backend composed of multiple internal stores MAY NOT commit
//! `write_batch` atomically across those stores.

use kenn_model::{
    DefRecord, EdgeRecord, FileDocsRecord, FileRecord, PackageRecord, SymbolDocsRecord,
    SymbolRecord,
};

/// Default batch threshold (total records across all six record types).
pub const DEFAULT_BATCH_SIZE: usize = 10_000;

/// One batched flush. `Vec`s sized roughly to the configured threshold.
#[derive(Debug, Default, Clone)]
pub struct WriteBatch {
    pub files: Vec<FileRecord>,
    pub packages: Vec<PackageRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub symbol_docs: Vec<SymbolDocsRecord>,
    pub file_docs: Vec<FileDocsRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
}

impl WriteBatch {
    /// A batch with each `Vec` pre-sized for a `threshold`-record flush.
    #[must_use]
    pub fn with_capacity(threshold: usize) -> Self {
        let cap = threshold.max(1);
        Self {
            files: Vec::with_capacity(cap / 8),
            packages: Vec::with_capacity(cap / 16),
            symbols: Vec::with_capacity(cap / 2),
            symbol_docs: Vec::with_capacity(cap / 8),
            file_docs: Vec::with_capacity(cap / 16),
            defs: Vec::with_capacity(cap / 2),
            edges: Vec::with_capacity(cap),
        }
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.files.len()
            + self.packages.len()
            + self.symbols.len()
            + self.symbol_docs.len()
            + self.file_docs.len()
            + self.defs.len()
            + self.edges.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Drop every accumulated record, keeping the `Vec` allocations for
    /// reuse by the next flush cycle.
    pub fn clear(&mut self) {
        self.files.clear();
        self.packages.clear();
        self.symbols.clear();
        self.symbol_docs.clear();
        self.file_docs.clear();
        self.defs.clear();
        self.edges.clear();
    }
}
