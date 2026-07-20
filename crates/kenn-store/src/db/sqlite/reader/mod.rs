//! `SQLite` reader (replace-lance-with-sqlite, task 1.3 / Phase 3).
//!
//! Holds a read-only async-sqlite connection pool over the published snapshot
//! (`code.db` main + `vector.db` attached). Queries run on the pool's
//! background threads, off the runtime workers. The pool + the per-closure
//! [`projection::SqliteConnRef`] live in [`projection`]; the inherent query
//! groups and the [`crate::api::Reader`] dispatch are split across the sibling
//! submodules.

mod code_links;
mod css_health;
mod fetch;
mod projection;
#[expect(
    clippy::module_inception,
    reason = "the `reader` submodule holds the SqliteReader pool dispatch; reader/reader is intentional"
)]
mod reader;
mod scan;
mod search;
mod traversal;

#[cfg(test)]
mod tests;

pub(crate) use projection::SqliteReader;
