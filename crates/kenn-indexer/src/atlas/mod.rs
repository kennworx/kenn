//! The `atlas` capability — a generated OKF v0.1 bundle (an agent-facing
//! structural map of the repo) written by `kenn index`. kenn is the structural
//! producer (deterministic facts); the agent enriches understanding in-context.
//!
//! - [`model`] — the domain structs a package concept carries.
//! - [`okf`] — OKF serialization (concept docs, `index.md`, `log.md`).
//!
//! The Reader-backed producer and the shared `finalize_atlas` wiring land here
//! next (tasks 3–5).

pub mod model;
pub mod okf;
pub mod producer;
