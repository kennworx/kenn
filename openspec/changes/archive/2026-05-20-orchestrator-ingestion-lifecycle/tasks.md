## 1. short_id partitioning and per-language registries

- [x] 1.1 Define the `short_id` partition encoding — high bits = `Language` discriminant, low bits = per-language counter; helpers to compose/inspect a partitioned id.
- [x] 1.2 Make `IdRegistry` per-ingester (one per language partition); remove the run-global shared registry threaded through `run_pipeline`.

## 2. The ingester → DB-writer channel

- [x] 2.1 Define the channel message type — `Begin` (stream meta) / the six record kinds / `End` (stream stats) — and a bounded channel whose capacity is a record count.
- [x] 2.2 Ingester path: parse → intern into the owned partition → build records → send `Begin`, records, `End`.
- [x] 2.3 DB-writer thread: drain the channel, accumulate into a `WriteBatch`, flush to redb and append per-batch to the Lance store; match `Begin`/`End` to detect clean completion vs a truncated (crashed) ingester.

## 3. Remove the Writer trait from kenn-store

- [x] 3.1 Remove `api::Writer` and `BatchingWriter` from `kenn-store`; backends expose ingestion / aggregate / finalize operations as public inherent methods.
- [x] 3.2 Expose the active backend as a concrete writer type with an `ActiveWriter` alias returned by `open_writer`; callers use the alias, not a backend module path.
- [x] 3.3 Retarget `aggregate.rs::compute_and_persist` and the `kenn-analyze` analysis hook to the concrete active writer (no trait object).
- [x] 3.4 Remove the `Writer` test doubles (`NullSink`, `CountingSink`, `VecWriter`); test ingesters by draining their channel into a `Vec` and the DB-writer against a temp backend.

## 4. Phase 1 — prepare

- [x] 4.1 Construct the backend (redb `Database`, Lance temp store) in the prepare phase, owned by the DB-writer thread; remove backend creation from any per-ingester `begin`.
- [x] 4.2 Add the phase-1 preflight: verify required ingester CLIs are available; fail the run in the prepare phase (before any store write) when one is missing.

## 5. Phases 2–4 orchestration

- [x] 5.1 Restructure `run_pipeline` into the four explicit phases; phase 2 spawns the DB-writer thread plus one ingester per language and drives them to completion.
- [x] 5.2 Run the aggregate phase after ingestion completes, then `finalize` as the single phase-4 step; ensure data becomes visible only at the publish point.

## 6. Verification

- [x] 6.1 Parallel-ingester test: several language ingesters → bounded channel → one DB-writer; partitions disjoint, the code graph holds the union, no lock contention.
- [x] 6.2 Backpressure test: a fast ingester against a slow DB-writer never exceeds the channel's record capacity in flight.
- [x] 6.3 Crash-safety test: an ingester that drops without `End` is detected as truncated; a failure before finalize leaves the previously published stores intact and queryable.
- [x] 6.4 Run the storage fixture harness and `cargo clippy --workspace --all-targets` — zero warnings.

## 7. Single-backend collapse

- [x] 7.1 Remove the `db_surreal` backend, the `surrealdb` dependency, and the `db_default` / `db_surreal` cargo features from every crate.
- [x] 7.2 Collapse `kenn-store/src/backends/db_default/` to `kenn-store/src/db/`; drop the `backends` module and all `#[cfg(feature = …)]` backend gating.
- [x] 7.3 Revise the `storage-backend-abstraction` spec — drop the cargo-feature backend selection, the cross-backend fixture / bench harness requirements, and the SurrealDB-behind-the-abstraction requirement.
