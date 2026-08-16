//! `list_documents` — the document axis of the graph, as a query.
//!
//! First-party non-code top-level directories: the ones that hold indexed files
//! but are not a code package (`openspec`, `docs`, `claude-plugins`, …). The
//! atlas renders these as `document` concepts; this answers the same question on
//! demand.
//!
//! This axis serves no file CONTENT — `kenn get source` and the markdown index
//! already do that. It exists so an agent can discover that these directories
//! are tracked concepts at all.

use std::collections::{BTreeMap, HashSet};

use kenn_indexer::atlas::domains::is_code_lang;
use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::QueryError;
use crate::types::ListResponse;

use crate::ctx::QueryCtx;
use crate::internal;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListDocumentsArgs {
    /// Restrict to one directory by name, as it appears in the atlas.
    #[serde(default)]
    pub document: Option<String>,
    /// Rows per response and the continuation cursor. The axis is computed
    /// whole and ordered deterministically, so this is a plain offset walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::types::Pagination>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DocumentView {
    /// The concept id — the atlas document this row corresponds to. First
    /// column: it is the handle a reader acts on.
    pub id: String,
    /// The directory name.
    pub title: String,
    /// Workspace-relative directory path.
    pub path: String,
    /// Indexed files under it.
    pub file_count: u64,
}

/// The pure selection: which top-level directories are non-code documents.
///
/// A directory is a document when it holds indexed files and NONE of them is in
/// a code language — code lives in packages, which the package axis covers.
/// Root-level files (`README.md`, `CLAUDE.md`) are not a document directory.
#[must_use]
pub fn document_views(files: &[kenn_store::FileRow], want: Option<&str>) -> Vec<DocumentView> {
    let code_dirs: HashSet<&str> = files
        .iter()
        .filter(|f| !f.external && is_code_lang(&f.language))
        .filter_map(|f| f.path.split('/').next())
        .collect();

    let mut per_dir: BTreeMap<&str, u64> = BTreeMap::new();
    for f in files.iter().filter(|f| !f.external) {
        let Some((top, _)) = f.path.split_once('/') else {
            continue; // a root-level file is not a document directory
        };
        if code_dirs.contains(top) {
            continue;
        }
        *per_dir.entry(top).or_default() += 1;
    }

    per_dir
        .into_iter()
        .filter(|(dir, _)| want.is_none_or(|w| w == *dir))
        .map(|(dir, file_count)| DocumentView {
            id: format!("documents/{}", dir.replace(['/', '\\'], "_")),
            title: dir.to_string(),
            path: dir.to_string(),
            file_count,
        })
        .collect()
}

/// List the workspace's first-party non-code directories.
pub async fn list_documents(
    ctx: &QueryCtx<'_>,
    args: &ListDocumentsArgs,
) -> Result<ListResponse<DocumentView>, QueryError> {
    let want = args.document.clone();
    let args_pagination = args.pagination.clone();
    let files = ctx.read.scan_files().await.map_err(internal)?;
    let items = document_views(&files, want.as_deref());
    let (items, next) =
        crate::support::page_axis_items(items, args_pagination.as_ref(), ctx.snapshot_id)?;
    Ok(ListResponse { items, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_store::FileRow;

    fn file(path: &str, language: &str) -> FileRow {
        FileRow {
            id: 0,
            path: path.into(),
            language: language.into(),
            test: false,
            external: false,
        }
    }

    /// A top-level directory holding indexed files, none of them code, is a
    /// document. A directory that holds code is a package's business, and a
    /// root-level file is not a directory at all.
    ///
    /// Mutation-checked: dropping the `code_dirs` exclusion admits `crates`.
    #[test]
    fn non_code_directories_are_documents_with_file_counts() {
        let files = vec![
            file("crates/kenn-store/src/lib.rs", "rust"),
            file("crates/kenn-store/README.md", "markdown"),
            file("docs/design.md", "markdown"),
            file("docs/api.md", "markdown"),
            file("openspec/specs/a/spec.md", "markdown"),
            file("README.md", "markdown"),
        ];
        let got = document_views(&files, None);
        let names: Vec<(&str, u64)> = got
            .iter()
            .map(|d| (d.title.as_str(), d.file_count))
            .collect();
        assert_eq!(
            names,
            vec![("docs", 2), ("openspec", 1)],
            "`crates` holds code so it is a package dir; the root README is not a directory"
        );
        assert_eq!(got[0].id, "documents/docs");
    }

    /// Naming one directory narrows to it.
    #[test]
    fn a_named_document_is_the_only_row() {
        let files = vec![
            file("docs/a.md", "markdown"),
            file("openspec/b.md", "markdown"),
        ];
        let got = document_views(&files, Some("openspec"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "openspec");
    }
}
