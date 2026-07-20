//! [`BatchSink`] — a language ingester's append surface.
//!
//! redb required one synchronous DB-writer thread, so every ingester
//! funnelled records through a bounded channel into it. The Lance store
//! is async and resolves concurrent appends itself, so each ingester
//! owns a `BatchSink` directly (design D9): the sink accumulates records
//! into a [`WriteBatch`] and, once the batch is full, appends it through
//! the ingester's own writer. No channel, no DB-writer thread.
//!
//! The ingester runs on a plain OS thread (no tokio context), so the
//! sink drives the async append with `Handle::block_on`.

use kenn_model::{
    DefRecord, EdgeRecord, FileDocsRecord, FileRecord, PackageRecord, SymbolDocsRecord,
    SymbolRecord,
};
use kenn_store::api::{DbError, WriteBatch};
use kenn_store::DbWriter;
use tokio::runtime::Handle;

/// Per-ingester batching append surface over the code-graph store.
pub struct BatchSink {
    /// This ingester's own writer (a clone of the run writer).
    writer: DbWriter,
    /// Runtime handle the ingester's OS thread drives appends through.
    handle: Handle,
    /// Records accumulated since the last append.
    batch: WriteBatch,
    /// Append once the batch reaches this many records.
    batch_size: usize,
    /// Total records appended through this sink.
    records: u64,
}

impl BatchSink {
    /// Build a sink that batches into `writer`, appending every
    /// `batch_size` records. `handle` drives the async append from the
    /// ingester's OS thread.
    #[must_use]
    pub fn new(writer: DbWriter, handle: Handle, batch_size: usize) -> Self {
        let batch_size = batch_size.max(1);
        Self {
            writer,
            handle,
            batch: WriteBatch::with_capacity(batch_size),
            batch_size,
            records: 0,
        }
    }

    /// Append the accumulated batch if it has reached `batch_size`.
    fn flush_if_full(&mut self) -> Result<(), DbError> {
        if self.batch.total() >= self.batch_size {
            self.flush()
        } else {
            Ok(())
        }
    }

    /// Append the accumulated batch now. A no-op when it is empty.
    fn flush(&mut self) -> Result<(), DbError> {
        if self.batch.is_empty() {
            return Ok(());
        }
        self.records += self.batch.total() as u64;
        self.handle.block_on(self.writer.write_batch(&self.batch))?;
        self.batch.clear();
        Ok(())
    }

    pub fn push_file(&mut self, f: FileRecord) -> Result<(), DbError> {
        self.batch.files.push(f);
        self.flush_if_full()
    }

    pub fn push_package(&mut self, p: PackageRecord) -> Result<(), DbError> {
        self.batch.packages.push(p);
        self.flush_if_full()
    }

    pub fn push_symbol(&mut self, s: SymbolRecord) -> Result<(), DbError> {
        self.batch.symbols.push(s);
        self.flush_if_full()
    }

    pub fn push_symbol_docs(&mut self, d: SymbolDocsRecord) -> Result<(), DbError> {
        self.batch.symbol_docs.push(d);
        self.flush_if_full()
    }

    pub fn push_file_docs(&mut self, d: FileDocsRecord) -> Result<(), DbError> {
        self.batch.file_docs.push(d);
        self.flush_if_full()
    }

    pub fn push_def(&mut self, d: DefRecord) -> Result<(), DbError> {
        self.batch.defs.push(d);
        self.flush_if_full()
    }

    pub fn push_edge(&mut self, e: EdgeRecord) -> Result<(), DbError> {
        self.batch.edges.push(e);
        self.flush_if_full()
    }

    /// Append every record from one transformed document.
    pub fn push_document_records(
        &mut self,
        files: impl IntoIterator<Item = FileRecord>,
        symbols: impl IntoIterator<Item = SymbolRecord>,
        symbol_docs: impl IntoIterator<Item = SymbolDocsRecord>,
        defs: impl IntoIterator<Item = DefRecord>,
        edges: impl IntoIterator<Item = EdgeRecord>,
    ) -> Result<(), DbError> {
        self.batch.files.extend(files);
        self.batch.symbols.extend(symbols);
        self.batch.symbol_docs.extend(symbol_docs);
        self.batch.defs.extend(defs);
        self.batch.edges.extend(edges);
        self.flush_if_full()
    }

    /// Append the final partial batch and report the total record count.
    pub fn finish(mut self) -> Result<u64, DbError> {
        self.flush()?;
        Ok(self.records)
    }
}
