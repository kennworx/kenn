//! Config-driven store layout — the `store-layout` capability.
//!
//! [`Layout`] is the single source of every store path. It resolves
//! its roots once, from [`kenn_config::Config`] plus the source root,
//! and exposes an accessor for every file the store touches — no other
//! component joins a path segment like `.kenn`, `local/`, or `findings/`
//! on its own.
//!
//! The store splits into **committed** and **derived** data:
//!
//! - `committed_root` — always `<source_root>/.kenn`, git-tracked, never
//!   relocatable. Holds the durable findings records
//!   `findings/<id>.md` and the committed `.gitignore`.
//! - `vectors_root` — committed vector sidecars (code + findings),
//!   defaults to `<committed_root>/vectors` and *relocatable* via
//!   `[vectors] location` (a path, or the keyword `"global"`). Holds
//!   `code/` and `findings/` sibling subdirectories, each containing
//!   `pack-{hash}.bin` (CI-produced, committed) and `seg-{hash}.bin`
//!   (dev-local, gitignored) files.
//! - `derived_root` — throwaway, gitignored, rebuilt by `kenn index`,
//!   and *relocatable* via `[layout] derived_root` (a path, or the
//!   keyword `"global"`). Defaults to `<committed_root>/local`. Holds
//!   the per-run directories, `live`, `index.lock`, and reader/embed
//!   bookkeeping.
//!
//! [`Store`] is a thin handle over a resolved `Layout`: `Store::open`
//! creates `derived_root` and the committed `.gitignore`, and exposes the
//! lifecycle paths.

mod gitignore;
mod resolve;
mod store;
mod types;

pub use store::{RunMeta, Store};
pub use types::{Layout, StoreError};

pub(crate) use gitignore::write_gitignore;
