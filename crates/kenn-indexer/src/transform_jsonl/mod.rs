//! Transform a JSONL frame stream into `kenn-model` records.
//!
//! Wire format: `indexers/frames.ts` (after wire-pkg-and-stubs).
//! Edges arrive pre-derived on the wire — we don't run SCIP-style edge
//! derivation here.
//!
//! ## Identifier mapping (consumer-side)
//!
//! Producer wire ids are run-local `u32`s; the consumer interns to its own
//! `ShortId` space.
//!
//! - `FileFrame.id` → `ShortId` via `IdRegistry::intern_file_with_seen`.
//! - `PackageFrame.id` → `ShortId` interned by `(name, version)`. Two
//!   wire ids collapse to one row when their packages share `(name,
//!   version)` (multi-target compilations of the same library).
//! - `SymbolFrame.id` / `StubFrame.id` → `ShortId` via
//!   `IdRegistry::intern_with_pub_id`. The intern key is salted with
//!   `pkg_short` so two different package versions declaring the same
//!   `pub_id` get distinct rows.
//!
//! ## Stub-then-upgrade and cross-wire-id dedup
//!
//! A `StubFrame` allocates a `ShortId` and buffers a minimal symbol record
//! in `pending_stubs`. A subsequent `SymbolFrame` with the SAME wire id is
//! the upgrade: the pending stub is evicted and the full record (with
//! `defs` row) is emitted in its place. Stubs whose upgrade never arrives
//! (typical for external symbols) flush at end-of-stream as bare rows
//! without a `defs` entry.
//!
//! A `SymbolFrame` with a NEW wire id whose `(key, pkg_short)` collides
//! with an existing symbol is a CROSS-WIRE-ID DEDUP, not a stub upgrade.
//! If `partial: true`, the additional declaration site is appended to
//! `defs`. If `partial: false`, the wire id is recorded in
//! `dup_sym_wires` so its outgoing edges are skipped (multi-target
//! source-shared duplicate emission).
//!
//! Module layout:
//! - [`ingest`] — error/stats types + the public entry points and read loop.
//! - [`stream`] — `handle_frame` dispatch + the `StreamState` interning
//!   machine + the end-of-job stub flush.
//! - [`records`] — pure record builders and the `kind`/`edge_kind` decoders.

mod ingest;
mod records;
mod stream;

pub use ingest::{
    ingest_jsonl_from_growing_file, ingest_jsonl_into_sink, JsonlIngestStats, JsonlTransformError,
};
pub use stream::flush_registry_stubs;

pub(crate) use ingest::{emit_ts_bench, JSONL_FAILED_ATTRIBUTION_CAP};
pub(crate) use records::{build_docs_record, edge_properties, file_doc_record, kind_from_str};
pub(crate) use stream::{handle_frame, StreamState};
