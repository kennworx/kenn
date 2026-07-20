## Context

Indexing today runs inside one `kenn-indexer::run_pipeline` body: discover the
workspace, stream each unit through an ingester into a `BatchingWriter<W:
Writer>`, force-flush, run the aggregation pass, call `end_run`. Preparation,
ingestion, aggregation, and finalization are not named phases — they are
interleaved statements.

The `Writer` trait (`begin_run` / `write_batch` / `end_run`) lives in
`kenn-store::api`, moved there from `kenn-indexer/src/sink.rs` and renamed.
But `begin → batch → end` is the *ingester's* run lifecycle, not a storage
concern.

Ingestion is also single-threaded against shared mutable state. The
`IdRegistry` — `(language, pub_id) → short_id` interning, monotonic
`short_id` counters, the cross-unit stub buffer — is one run-global object
threaded `&mut` through every unit. Parallelizing parsing while that object
is shared means locking it on the hot path.

Parallel parsing is a hard requirement: the external ingester subprocesses
are CPU-bound and the JSONL/SCIP deserialization is non-trivial.

## Goals / Non-Goals

**Goals:**
- An explicit 4-phase orchestrator — prepare, ingest, aggregate, finalize.
- Parallel ingestion with **zero shared mutable state** between ingesters.
- Predictable, bounded ingestion memory.
- A single owner of the storage backend.
- The run lifecycle owned by `kenn-indexer`, not `kenn-store`.

**Non-Goals:**
- The vector indexer / embedding producer. Phase 3 runs the aggregate pass
  only; embeddings are a later addition to phase 3.
- The `Reader` trait and the search surface — unchanged.
- The on-disk format of either store.
- Building the nested multi-ingester-per-language orchestrator now — the
  architecture allows it (D5); this change does not implement it.

## Decisions

### D1 — Four explicit phases, owned by the orchestrator

`kenn-indexer` runs an index as four named, ordered phases:

1. **Prepare** — create `.kenn/{local/,knowledge/}` and the run directories;
   construct the backend (the redb `Database` and the Lance temp store);
   preflight that required ingester CLIs are available.
2. **Ingest** — spawn the DB-writer thread and one ingester per language;
   ingesters parse in parallel and stream records to the DB-writer.
3. **Aggregate** — compute the aggregate graph from the redb code graph and
   write the `aggregate_*` / `analysis_*` tables.
4. **Finalize** — compact the Lance fragments, build indexes, and atomically
   publish both stores (Lance directory swap into `.kenn/knowledge/`; redb
   `building/ → snapshots/` + `live` symlink flip) — the point data becomes
   visible.

The orchestrator owns phases 1, 3, 4; phase 2 it delegates to ingesters.

### D2 — `short_id` is partitioned by language; no global `IdRegistry`

`short_id` (a `u32`) is partitioned: the top bits hold the `Language`
discriminant, the low bits a per-language counter. `Language` is a closed,
compile-time-known enum, so the partition count is fixed.

Each ingester owns one language partition and its own `IdRegistry` and
`StreamState`. There is no run-global registry and no cross-ingester shared
state.

This is sound because `pub_id` is language-prefixed (`<lang>:<key>`), so two
languages can never produce the same identity — cross-language dedup is
impossible by construction. Within a language, dedup (`by_pub_id`,
`packages`, `files`) and the cross-unit stub buffer stay inside that one
ingester's registry. The invariant is **one ingester == one language == one
partition** (see D5 for the multi-ingester-per-language case).

Bonus: each partition is interned in its own stream order, independently of
the others, so `short_id` is deterministic again — unlike a single registry
fed by interleaved parallel streams.

Alternative considered — a content-addressed `short_id` (hash of `pub_id`),
which needs no coordination at all. Rejected: a `u32` hash collides well
below realistic symbol counts, and widening `short_id` to `u64` ripples
through redb keys and the Lance schema.

### D3 — Ingester → DB-writer over a bounded record channel

Each ingester parses, interns into its own partition, builds records, and
sends them to a **single DB-writer thread** over a bounded channel. The
channel carries built records plus `Begin` / `End` markers.

The channel is **bounded by record count** — `capacity_records ×
record_size` — not by batches. Batch size is an ingester-side choice, so
bounding by batch count makes memory unpredictable; bounding by item count
does not. Backpressure is the blocking send when the channel is full.

The DB-writer thread is the **sole owner** of the redb `Database` and the
Lance temp store. It accumulates channel records into a `WriteBatch`, flushes
to redb, and appends per-batch to the Lance store (keeping the Lance file
count down). Because there is exactly one writer, there are no locks on
either store.

`Begin` / `End` are not needed to detect "no more data" — that is the
channel closing when every `Sender` drops. They carry per-stream metadata
(the `MetaFrame`) and let the DB-writer distinguish a **clean finish** (an
`End` was seen) from a **crashed ingester** (its `Sender` dropped with no
`End`).

### D4 — No ingestion-lifecycle trait; `kenn-store` exposes inherent ops

`kenn-store` no longer defines a `Writer` trait, and `kenn-indexer` defines
no sink trait either — the ingester→writer seam is the channel. The backend
is selected at compile time (one concrete type per build), so the DB-writer
thread calls the backend's **public inherent methods** directly; no trait is
needed for polymorphism.

`kenn-store` exposes the active backend as a concrete writer type (and a
`ActiveWriter` type alias from `open_writer`) with public inherent
operations — `write_batch`, the aggregate scans, `write_aggregate_tables` /
`write_analysis_tables`, `finalize`. `kenn-store` keeps owning the
*mechanism* (the temp store, compaction, the publish swap); it stops owning
any *trait*.

`BatchingWriter` is removed — the DB-writer thread does the batching inline
as it drains the channel. Test doubles (`NullSink`, `CountingSink`,
`VecWriter`) are removed: the channel is the test seam — ingesters are
tested by draining their channel into a `Vec`, the DB-writer by a temp
backend.

### D5 — Recursive composition for multiple ingesters per language

The top level always sees one ingester per language. A language that needs
multiple parallel ingesters runs a **nested orchestrator** with its own
channel that merges them into one logical per-language ingester — the same
shape, recursed. The per-language partition (D2) is therefore never
violated: the nested orchestrator interns into the single language partition
on behalf of its sub-ingesters.

This change does not implement the nested case; D2–D4 are structured so it
can be added later without disturbing the top level.

### D6 — Phase-1 preflight for ingester CLIs

Phase 1 verifies the ingester CLIs the run needs are available before any
store is written, so a missing toolchain fails fast in prepare rather than
mid-ingest with a half-written store.

## Risks / Trade-offs

- **Coordination with `lance-search-backend`** — that change's §10 creates
  the Lance temp store in `begin_run`; this change moves creation to phase 1
  and makes the DB-writer the sole owner. → Mitigation: sequence this change
  after `lance-search-backend`; the design assumes §10 is in.
- **The one-ingester-per-language invariant** — D2 depends on it. → Mitigation:
  D5 — the nested-orchestrator recursion keeps the invariant intact for the
  multi-ingester case; it is structural, not a special case in the writer.
- **`u32` partition budget** — splitting `short_id` reduces per-language
  capacity to the low bits (e.g. ~16 languages → 4 bits → ~268M ids per
  language per entity type). → Mitigation: ample for realistic repositories;
  `short_id` was already `u32`-bounded.
- **Broad blast radius** — removing the `Writer` trait touches `kenn-store`,
  `kenn-indexer`, `kenn-analyze`, `kenn-cli`, `kenn-mcp`. → Mitigation:
  `Reader`, `WriteBatch`, and row/error types are untouched; the change is
  mechanical and the cross-backend fixture harness gates it.

## Migration Plan

- Internal refactor: no on-disk format change for either store, no data
  migration. A snapshot indexed before and after this change is
  byte-compatible.
- Land after `lance-search-backend`. Update `storage-backend-abstraction` to
  drop the `Writer`-trait requirement; add the `indexing-orchestrator`
  capability.
- Rollback: revert the change; the backends still function (their operations
  exist either way) and the indexer reverts to the single-`run_pipeline`
  body.

## Open Questions

- None blocking. The vector indexer / embedding producer slots into phase 3
  when it lands (deferred — Non-Goals).
