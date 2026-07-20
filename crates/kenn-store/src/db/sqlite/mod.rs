//! `SQLite` storage backend (replace-lance-with-sqlite).
//!
//! A published snapshot is two databases — `code.db` (the code graph,
//! bulk-scanned into a resident projection at open) and `vector.db`
//! (FTS5 identifier/prose search + a `sqlite-vec` `vec0` table for vector
//! KNN). Committed embedding vectors live in the `.kenn/vectors/` sidecar,
//! reconciled into `vec0` at index time and filled by the embed pass.
//! Findings are records-based (`.kenn/findings/<id>.md`), searched via a
//! transient in-memory FTS5 index plus the findings sidecar. See
//! `design.md` D1–D9.

mod handle;
mod reader;
mod register;
mod schema;
mod writer;

pub use handle::{DbConn, DbReader, DbWriter};
pub(crate) use register::ensure_vec_extension;
