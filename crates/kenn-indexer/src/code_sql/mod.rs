//! Code → SQL table references: the tables a function's own source names.
//!
//! Code is the third source of table references, after `.sql` files and
//! SQL carried in markup. Without it a table node is reachable from
//! migrations and query files and nothing else, so "what code reads this
//! table" — the question indexing SQL exists to answer — has no answer.
//!
//! Two pure pieces here, both testable without a store:
//! - [`literals`] recovers string-literal contents from source text.
//! - [`attribute`] places a literal on the innermost symbol containing it.

pub mod attribute;
pub mod ingest;
pub mod literals;
pub mod resolve;
