//! In-container toolchain provisioning.
//!
//! Every indexer image runs this crate's binary as its ENTRYPOINT. It reads the
//! toolchain version the workspace pins for itself, provisions that version into
//! the mounted cache volume if it is not already there, and then `exec`s the
//! real indexer.
//!
//! # Why it lives in the container rather than on the host
//!
//! kenn calls `docker` only to build images; it does not orchestrate downloads.
//! Doing the work at indexer start also means it works identically for the three
//! languages that run third-party indexers (`rust-analyzer`, `scip-go`,
//! `scip-python`) — those have no code of ours to hook, so a kenn-authored
//! entrypoint in front of them is the only uniform place this can happen.
//!
//! Being a Rust binary rather than a shell script is what keeps the images
//! `FROM scratch`: no `sh`, no `curl`, no package manager.

pub mod cache;
pub mod fetch;
pub mod pin;
pub mod resolve;
pub mod run;
