//! Public surface that the active backend implements and that callers
//! (`kenn-cli`, `kenn-mcp`, `kenn-indexer`) consume.
//!
//! * [`Reader`] (async) — every read operation MCP needs.
//! * [`WriteBatch`] — the value type the backend's `write_batch`
//!   operation consumes. There is no `Writer` trait: the backend is
//!   selected at compile time, so the indexer's DB-writer thread calls
//!   the concrete writer's public inherent operations directly.
//!
//! Plus the row / result types in [`types`].
//!
//! One important non-promise: a backend composed of multiple internal
//! stores MAY NOT commit `write_batch` atomically across those stores.
//! A reader observing a snapshot mid-flush MAY see partial state. The
//! documented recovery posture is re-ingest from the source corpus.

pub mod reader;
pub mod types;
pub mod writer;

pub use reader::Reader;
pub use types::{
    BlendedFileRow, BlendedHit, BlendedSymbolRow, CodeSymbolHit, DbError, DefLineRow, DefRow,
    FileRow, FoundSymbolRow, LinkDiagnosticRow, MatchKind, PackageRow, RankedSymbolRow, StatRow,
    SymbolDocsRow, SymbolRow, WriterOptions,
};
pub use writer::{WriteBatch, DEFAULT_BATCH_SIZE};
