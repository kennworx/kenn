//! `SQLite` ingest writer (replace-lance-with-sqlite, tasks 2.1 / 2.3).
//!
//! Maps a [`WriteBatch`] of typed records 1:1 into the `code.db` tables in
//! one transaction. The knowledge-row derivation (which joins symbols+docs
//! across batches) and FTS5/`vec0` population happen at `finalize` from the
//! fully-populated graph tables — not here — so cross-batch ordering of a
//! symbol and its doc rows doesn't matter (design D2/D5). The struct, batch
//! write, and helpers live in [`core`]; `finalize` and the aggregate /
//! analysis passes are split across the sibling submodules.
//!
//! [`WriteBatch`]: crate::api::WriteBatch

mod aggregate;
mod core;
mod finalize;

#[cfg(test)]
mod tests;

pub(crate) use core::SqliteWriter;
