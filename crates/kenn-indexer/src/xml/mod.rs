//! XML indexing: the generic element walker and the `.xml` producer.
//!
//! Deliberately vocabulary-agnostic — no framework element name, attribute
//! name, or namespace URI appears here. A workspace supplies whatever meaning
//! it needs as configuration.

pub mod ingest;
pub mod parse;
