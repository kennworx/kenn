//! Storage backend factories — the public `open_writer` / `open_reader`
//! entry points. Each is a thin wrapper that names the engine's
//! constructor and (for `open_reader`) validates the snapshot's
//! `meta.json` invariants before any data flows.

use crate::api::types::{DbError, WriterOptions};
use crate::db::{DbReader, DbWriter};
use crate::meta::{check_backend_marker, check_schema_version};

/// Open a writer at `dir` with the given options.
pub async fn open_writer(
    dir: &std::path::Path,
    options: WriterOptions,
) -> Result<DbWriter, DbError> {
    DbWriter::create(dir, options)
}

/// Open a reader against a published `snapshot/` directory.
pub async fn open_reader(snapshot: &std::path::Path) -> Result<DbReader, DbError> {
    let _ = check_backend_marker(snapshot)?;
    let _ = check_schema_version(snapshot)?;
    DbReader::open(snapshot).await
}

/// Construct a reader over a writer's in-flight build snapshot.
///
/// Provided for in-process tests that need to read from a writer's
/// snapshot before publish. Opens the writer's run directory read-only;
/// the writer's committed rows are visible to the second connection.
#[doc(hidden)]
pub async fn reader_from_writer(writer: &DbWriter) -> Result<DbReader, DbError> {
    DbReader::open(writer.dir()).await
}
