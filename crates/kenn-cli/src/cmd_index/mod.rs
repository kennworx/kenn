//! `kenn index` — full orchestration of one indexer run.
//!
//! Sequence:
//! 1. Open `Store`, run `recover` to clean orphan `building/`
//! 2. Compute current `StalenessKey`; if it matches the live snapshot's
//!    recorded key (and `--force` is unset), skip the run
//! 3. `begin_indexing` (acquires flock, creates `building/`)
//! 4. Build `IndexerDriver` from config; open the active writer at `building/`
//! 5. `run_pipeline` ingests every emitted SCIP through the sink
//! 6. Write `meta.json` (counts + staleness key + regression warnings) and
//!    `report.json` (per-unit reports) into `building/` and `runs/<id>/`
//! 7. `publish` flips `live`; `gc` reclaims older snapshots

mod core;
pub use core::*;
