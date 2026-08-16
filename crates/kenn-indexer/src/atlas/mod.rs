//! The `atlas` capability — a generated OKF v0.1 bundle (an agent-facing
//! structural map of the repo) written by `kenn index`. kenn is the structural
//! producer (deterministic facts); the agent enriches understanding in-context.
//!
//! - [`model`] — the domain structs a package concept carries.
//! - [`okf`] — OKF serialization (concept docs, `index.md`, `log.md`).
//!
//! [`coupling`], [`domains`], and [`contracts`] hold the per-axis SELECTION
//! rules, shared so the producer and the query surface can never disagree about
//! the same snapshot; render caps stay in [`producer`].

pub mod contracts;
pub mod coupling;
pub mod domains;
pub mod model;
pub mod okf;
pub mod producer;
pub mod tables;
