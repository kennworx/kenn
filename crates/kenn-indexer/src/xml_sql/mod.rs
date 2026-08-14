//! The XML↔SQL bridge: a post-producer barrier step joining XML elements to
//! the table graph.
//!
//! `index-sql` reads `.sql` files and `index-xml` reads XML structure, and
//! neither can see the other. Measured during design on a real repository,
//! that is where most of a schema lives: 25 tables declared by `CREATE TABLE`
//! in `.sql`, against 103 named by an XML attribute and 1008 elements carrying
//! SQL in their bodies. Two producers, both correct, and the join between them
//! missing.
//!
//! The join cannot live in either. A `<select>` body is SQL the XML producer
//! has no business parsing, sitting in a file the SQL producer never opens. So
//! it runs where the pipeline already joins its parallel producers, alongside
//! the markdown, stylesheet and HTML resolutions — the same shape, not a new
//! mechanism.

pub mod ingest;
pub mod resolve;
