//! The committed vector sidecar — `fingerprint -> vector` storage
//! (`incremental-embedding`, KVS2 format).
//!
//! Embeddings are persisted as a content-addressed set of immutable
//! files. Each file is named `pack-{xxh3_64(bytes):016x}.bin` (CI-
//! produced, committed via git, the canonical record) or
//! `seg-{xxh3_64(bytes):016x}.bin` (dev-local, gitignored, incremental
//! since the last `git pull`). Both prefixes share one on-disk byte
//! layout — the prefix distinguishes only the producer's role, not the
//! file format. The fingerprint is the xxh3-64 of a symbol's embeddable
//! text; vectors are stored int8-quantized (per-vector symmetric scalar)
//! at full model dimension.
//!
//! Files are **content-addressed and append-only** (design D9/D10):
//!
//! - Same `(text, model)` set → same byte payload → same content hash →
//!   same filename. Two machines or two re-runs converge automatically;
//!   sync engines (Syncthing, Dropbox, iCloud) see "same file on both
//!   sides" without conflict.
//! - No file is ever rewritten. Compaction is gone. Dead vectors
//!   (whose source symbol was deleted) accumulate harmlessly until a
//!   future `kenn gc` maintenance pass removes them.
//! - The only directory-entry change the indexer is permitted to make
//!   beyond "create a new content-addressed file" is the `--repack`
//!   seg-to-pack rename (D13), which preserves bytes — only the
//!   directory entry changes name.
//!
//! Each file carries a 16 B fixed header + sorted fingerprint list +
//! int8 payload. Per-file count is capped at `MAX_ENTRIES = 510`, so a
//! full file's header fits in one 4 KB OS page — a reader can build the
//! global fp set by mmap'ing just one page per file.
//!
//! Module map:
//! - `quant` — fingerprint hash + int8 quantization (`QuantVector`).
//! - `segment` — on-disk byte layout (`Segment::encode` / `::decode`)
//!   plus the format-level constants and the page-budget cap.
//! - `io` — writers (`append_vectors`, `promote_segs_to_packs`),
//!   reader (`load_vectors`, `load_reuse_map`), atomic write, and the
//!   `WriterPrefix` enum.
//! - `manifest` — `manifest.toml` schema and IO (`Manifest::current`,
//!   `read`, `write`).
//! - `generation` — generation-namespaced directory layout, the legacy
//!   flat-dir fallback, last-access stamps, and the LRU size-cap GC
//!   (`shared-vector-cache`).

pub(crate) mod generation;
pub(crate) mod io;
pub(crate) mod manifest;
pub(crate) mod quant;
pub(crate) mod segment;

pub use generation::{
    code_generation_dir, current_model_id, findings_generation_dir, gc_vector_cache, sidecar_dirs,
    VectorsGcReport,
};
pub use io::promote_segs_to_packs;

pub(crate) use io::{append_vectors, load_reuse_map_with_legacy, WriterPrefix};
pub(crate) use manifest::{Manifest, CODE_TEXT_RECIPE, FINDING_TEXT_RECIPE};
pub(crate) use quant::{fingerprint, QuantVector};
