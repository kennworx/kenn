//! Document-level transform: SCIP `Document` → `kenn_model` records.
//!
//! Task 4.3 lives here. Section-5 edge derivation calls into this module's
//! `IdRegistry` to translate SCIP symbol strings into the same `short_id`s
//! emitted at `SymbolRecord` time.
//!
//! Module layout:
//! - [`lang`] — `TransformError` + SCIP/extension language detection.
//! - [`registry`] — `IdRegistry` interning + the cross-stream stub buffer.
//! - [`naming`] — `Kind` derivation, stub interning, name/test-descriptor
//!   helpers derived from SCIP symbol strings.
//! - [`document`] — the per-`Document` transform that ties them together.

mod document;
mod lang;
mod naming;
mod registry;

pub use document::{transform_document, TransformedDocument};
pub use lang::{language_from_path, language_from_scip, transformer_for, TransformError};
pub use naming::{
    derive_kind_with_source, intern_symbol_with_stub, is_test_descriptor, parent_scip_symbol,
    KindSource,
};
pub use registry::IdRegistry;

pub(crate) use naming::{derive_display_name, derive_kind, derive_short_name};
