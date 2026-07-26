//! TOON (Token-Oriented Object Notation) for the query CLI — the flat-table
//! case only.
//!
//! TOON is used for exactly one shape: an object whose fields are scalars or a
//! list of uniformly-typed, non-nested objects (`items[N]{cols}:` + rows). That
//! is what `kenn packages` and the other listings are. Anything else — a nested
//! object (`kenn overview`, `kenn get`), an array whose elements nest — is NOT
//! TOON: [`write_table`] returns `Err` and the caller falls back to pretty JSON.
//!
//! [`write_table`] runs the value through a [`serde::Serializer`] that streams
//! TOON straight to the caller's writer. serde visits struct fields in
//! declaration order, so the column order is set where the view struct is
//! defined — never alphabetized. Byte-compatibility with the `toon` crate (for a
//! flat, already-sorted struct) is pinned by `tests::matches_upstream_toon`.
//!
//! A wrapper's own scalar fields (`next`, `targets`, …) are NOT part of the
//! table — `table` drops them and `crate::render` prints them beneath it.

mod element;
mod grammar;
mod table;
#[cfg(test)]
mod tests;

pub use table::write_table;
