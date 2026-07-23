//! Indexer-driver traits + the cross-language `IndexerDriver` orchestrator.
//!
//! Two distinct shapes live here, intentionally not unified:
//!
//! - [`ScipDriver`] is per-unit: produces a `.scip` file for one
//!   compilation unit at a time. SCIP-family indexers (rust-analyzer,
//!   scip-typescript) ship this way; the pipeline calls `discover_units`
//!   then loops `run_unit`.
//! - [`JsonlIndexer`] is workspace-wide: streams JSONL frames on stdout
//!   for an entire workspace in one process. `kenn-dotnet` is the only
//!   impl today. The indexer owns its project discovery and scheduling;
//!   the pipeline ingests stdout frame-by-frame.
//!
//! Module layout:
//! - [`contract`] — the traits and value types every driver shares.
//! - [`walk`] — directory walkers the drivers use for discovery.
//! - [`orchestrator`] — the `IndexerDriver` that runs every driver.

mod contract;
mod orchestrator;
mod walk;

mod dotnet;
mod go;
mod python;
mod rust;
mod swift;
mod typescript;

#[cfg(test)]
mod tests;

pub use contract::{
    DriverError, JsonlIndexer, JsonlOutcome, ScipDriver, ScipOutcome, StderrCapture, Unit,
};
pub use dotnet::KennDotnet;
pub use go::ScipGo;
pub use orchestrator::IndexerDriver;
pub use python::ScipPython;
pub use rust::RustAnalyzer;
pub use swift::KennSwift;
pub use typescript::KennTs;

pub(crate) use contract::{container_arg, error_reason, spawn_stderr_capture};
pub(crate) use orchestrator::make_scip_output_path;
pub(crate) use walk::walk_for_language;
