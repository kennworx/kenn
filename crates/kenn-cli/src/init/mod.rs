//! `kenn init` internals: detect what languages a workspace contains, decide
//! which are indexable here, and author a `kenn.toml` that fits.

pub mod author;
pub mod detect;
pub mod report;
