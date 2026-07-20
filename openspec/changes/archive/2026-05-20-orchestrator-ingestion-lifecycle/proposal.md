## Why

The `begin → write_batch → end` run lifecycle lives in `kenn-store` as the
`api::Writer` trait — but it was moved there from `kenn-indexer/src/sink.rs`
and renamed, and that sequence is the *ingester's* state machine, not a
storage concern. `kenn-store` is the storage layer; it should expose
operations, not own a run lifecycle.

Two further pressures: the indexing flow has no explicit, named phase
structure — preparation, ingestion, aggregation, and finalization are
tangled inside one `run_pipeline` body — and the recently-built knowledge
store lifecycle puts temp-store creation inside `begin_run`, which breaks
the moment more than one ingester each calls `begin` over a shared store.

## What Changes

- Introduce an explicit **4-phase orchestrator** owned by `kenn-indexer`:
  **prepare → ingest → aggregate → finalize**.
- **BREAKING**: remove the `Writer` run-lifecycle trait from `kenn-store`.
  The ingester→writer seam becomes a **bounded record channel**; there is
  no sink trait. The backend exposes its operations as public inherent
  methods, reached through the `ActiveWriter` type alias.
- **BREAKING**: collapse to a **single storage backend**. The legacy
  `SurrealDB` backend, the `surrealdb` dependency, and the
  `db_default` / `db_surreal` cargo features are removed; the Lance +
  redb backend moves from `kenn-store/src/backends/db_default/` to
  `kenn-store/src/db/` and all `#[cfg(feature = …)]` backend gating is
  dropped.
- **Partition `short_id` by language** — high bits = the `Language`
  discriminant — so each ingester interns into its own partition with its
  own `IdRegistry`. This eliminates the run-global registry and **all**
  cross-ingester shared mutable state.
- Run **one ingester per language**, in parallel; each parses, interns,
  builds records, and streams them to a **single DB-writer thread** over a
  bounded channel. A language wanting multiple ingesters recurses — a
  nested orchestrator + channel presenting as one logical ingester.
- The DB-writer thread is the **sole owner** of the redb + Lance backend;
  it batches channel records and is the only writer — no locks.
- Move backend creation into **phase 1** (prepare); `finalize` is a single
  **phase-4** step.
- `Begin` / `End` channel markers track clean completion vs a crashed
  ingester. `BatchingWriter` and the `Writer` test doubles are removed —
  the channel is the test seam.
- Add a phase-1 **preflight**: required ingester CLIs are available before
  ingestion starts.

## Capabilities

### New Capabilities

- `indexing-orchestrator`: the 4-phase indexing lifecycle (prepare,
  ingest, aggregate, finalize) — phase ownership and ordering, the
  language-partitioned `short_id` / per-ingester registry model, the
  bounded ingester→DB-writer record channel, `Begin`/`End` completion
  tracking, and the phase-1 preflight.

### Modified Capabilities

- `storage-backend-abstraction`: the `Writer` run-lifecycle trait is
  removed from `kenn-store`; the backend exposes its operations as public
  inherent methods on the concrete writer type and no longer owns an
  ingestion-lifecycle trait. The capability also collapses to a single
  backend — the cargo-feature backend selection, the cross-backend
  fixture / bench harness requirements, and the SurrealDB-behind-the-
  abstraction requirement are removed. `Reader` is unaffected.

## Impact

- **`kenn-store`** — `api::Writer` and `BatchingWriter` removed;
  `DefaultWriter` exposes ingestion + aggregate + finalize operations as
  public inherent methods (`begin_run` folded into `create`, `end_run`
  renamed `finalize`), reached via the `ActiveWriter` alias. The
  `db_surreal` backend, the `surrealdb` dependency, the `backends`
  module, and the cargo features are deleted; the backend lives under
  `src/db/`. `api::Reader`, `WriteBatch`, row/error types unchanged.
- **`kenn-indexer`** — `short_id` partitioning + per-ingester
  `IdRegistry`; the bounded record channel and the DB-writer thread;
  `run_pipeline` restructured into the four explicit phases;
  `compute_and_persist` retargeted to the concrete active writer.
- **`kenn-analyze`** — the `write_analysis_tables` caller retargets from
  the `Writer` trait to the concrete active writer.
- **`kenn-cli` / `kenn-mcp`** — `NullSink`, the `Writer for CountingSink`
  test double, and the pipeline call sites updated; the channel becomes
  the test seam.
- **Interacts with `lance-search-backend`** — that change's §10 puts
  Lance temp-store creation in `begin_run`; this change relocates it to
  phase 1 and makes the DB-writer the sole backend owner. This change
  lands after.
