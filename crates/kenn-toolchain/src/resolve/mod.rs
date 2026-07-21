//! Turning a [`Pin`](crate::pin::Pin) into a concrete downloadable artifact,
//! using each vendor's published release metadata.
//!
//! # Why metadata rather than a URL template
//!
//! Artifact URLs are not derivable. .NET's live under
//! `/dotnet/Sdk/<version>/dotnet-sdk-<version>-<rid>.tar.gz` but the *release*
//! that contains a given SDK is not its own version (SDK 9.0.308 ships in
//! release 9.0.11), and Rust's filenames use the release version while the
//! component's own `version` field says something else entirely (`cargo` reports
//! 0.98.0 inside the 1.97.1 release). Guessing works until it doesn't, and then
//! it fails as a 404 months later rather than at review time.
//!
//! The metadata is also where the checksum lives, so reading it is not extra
//! work — it is the only way to satisfy verification at all.
//!
//! # What each vendor actually publishes
//!
//! | language | URL | digest |
//! |---|---|---|
//! | rust | absolute, in the channel manifest | SHA-256 |
//! | dotnet | absolute, in `releases.json` | **SHA-512** |
//! | go | filename only, fixed base | SHA-256 |
//! | node | filename only, from `SHASUMS256.txt` | SHA-256 |
//! | python | absolute, in uv's index | SHA-256 |
//! | swift | **neither** — see [`resolve`] | — |

mod common;
mod dotnet;
mod go;
mod node;
mod python;
mod rust;
mod swift;

#[cfg(test)]
mod testutil;

pub use common::{default_pin, resolve, Arch, Install, ResolveError, Resolved, LATEST};
