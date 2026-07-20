//! The per-`Document` transform: walk one SCIP `Document` into
//! `kenn_model` records (file, symbols, docs, defs, edges) plus the
//! Rust file-doc extraction and definition-occurrence prepass it relies on.

mod walk;

#[cfg(test)]
mod tests;

pub use walk::{transform_document, TransformedDocument};

#[cfg(test)]
pub(crate) use walk::extract_rust_file_docs;
