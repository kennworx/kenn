//! Per-snapshot `meta.json` invariants: the active-backend marker and
//! the store-schema version. Both are stamped at `publish` time by the
//! lifecycle layer and verified by [`open_reader`](crate::open_reader)
//! before any data is read — a mismatch causes a clear
//! [`DbError`](crate::DbError) rather than a cryptic engine-level error.

use crate::api::types::DbError;

/// Backend marker recorded in each snapshot's `meta.json`. A snapshot
/// whose marker disagrees causes [`open_reader`](crate::open_reader) to
/// fail with a clear [`DbError::Backend`] message rather than a cryptic
/// engine-level "file corrupt" error.
pub const ACTIVE_BACKEND: &str = "sqlite";

/// Schema version recorded in each snapshot's `meta.json`. A snapshot
/// whose recorded version disagrees with the binary's compiled-in value
/// causes [`open_reader`](crate::open_reader) to fail with
/// [`DbError::SchemaMismatch`] — old indexes built under a different
/// schema cannot be trusted, and the caller must reindex.
///
/// Pre-versioning snapshots have no `schema_version` field; they are
/// treated as version `1` per the `store-layout` requirement.
pub const STORE_SCHEMA_VERSION: u32 = 4;

/// Inspect a snapshot directory's `meta.json` for `schema_version`.
///
/// Returns `Ok(persisted)` when the file is present and the value
/// matches [`STORE_SCHEMA_VERSION`]. Returns [`DbError::SchemaMismatch`]
/// when the value is present but disagrees (treating a missing field as
/// `1`, per the `store-layout` requirement).
///
/// Returns `Ok(STORE_SCHEMA_VERSION)` when `meta.json` is absent — the
/// snapshot has not been published through the lifecycle's stamp-then-flip
/// protocol, so version enforcement does not apply (test fixtures and
/// raw-`open_writer` callsites both land here). The lifecycle layer
/// already refuses to publish a run without `meta.json`, so the
/// "absent" branch never reaches a real reader on a properly-published
/// snapshot.
pub fn check_schema_version(snapshot: &std::path::Path) -> Result<u32, DbError> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct MetaSchema {
        schema_version: Option<u32>,
    }
    let meta_path = snapshot.join("meta.json");
    let bytes = match std::fs::read(&meta_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(STORE_SCHEMA_VERSION);
        }
        Err(e) => return Err(DbError::Io(e)),
    };
    let persisted = serde_json::from_slice::<MetaSchema>(&bytes)
        .ok()
        .and_then(|m| m.schema_version)
        .unwrap_or(1);
    if persisted == STORE_SCHEMA_VERSION {
        Ok(persisted)
    } else {
        Err(DbError::SchemaMismatch {
            persisted,
            expected: STORE_SCHEMA_VERSION,
        })
    }
}

/// Inspect a snapshot directory's `meta.json` for the `backend` field.
/// Returns `Ok(None)` if the file is missing or the field absent
/// (legacy / pre-marker snapshots). Returns `Err` with both names if
/// the field is present and disagrees with [`ACTIVE_BACKEND`].
pub fn check_backend_marker(snapshot: &std::path::Path) -> Result<Option<String>, DbError> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct MetaBackend {
        backend: Option<String>,
    }
    let meta_path = snapshot.join("meta.json");
    let bytes = match std::fs::read(&meta_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(DbError::Io(e)),
    };
    let parsed: MetaBackend = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    match parsed.backend {
        None => Ok(None),
        Some(found) => {
            if found == ACTIVE_BACKEND {
                Ok(Some(found))
            } else {
                Err(DbError::Backend(format!(
                    "snapshot was built by `{found}` but this build is `{ACTIVE_BACKEND}`. \
                     Re-index to rebuild the store."
                )))
            }
        }
    }
}
