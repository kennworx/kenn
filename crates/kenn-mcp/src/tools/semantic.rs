//! Semantic-search + source-retrieval tools.

use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{McpError, McpErrorCode};
use crate::types::{
    clamp_top_k_page, NotFoundHint, RankedFindingView, SearchHitRef, SemanticSearchResponse,
    SingleResponse, SourceView,
};

use super::{
    db_to_mcp, embed_query, finding_to_view, hit_to_ref, internal, slice_lines, split_public_id,
    ServerState,
};

// ── KNOWLEDGE LAYER ─────────────────────────────────────────────────────────

/// Scope for [`semantic_search`] — which corpora to rank.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    /// Code symbols only.
    Code,
    /// Findings only.
    Findings,
    /// Both corpora (the default).
    #[default]
    Both,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SemanticSearchArgs {
    pub query: String,
    /// `code` | `findings` | `both`. Defaults to `both`.
    #[serde(default)]
    pub scope: Option<SearchScope>,
    /// Per-corpus rows per response.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Include test-file symbols in the code arm. Default false. (The findings
    /// arm has no test dimension and is unaffected.)
    #[serde(default)]
    pub include_tests: Option<bool>,
    /// Include external (stdlib / vendored) symbols in the code arm. Default false.
    #[serde(default)]
    pub include_external: Option<bool>,
}

/// Blended (BM25 + vector) search over code symbols and/or findings,
/// scopeable. The query is embedded once and shared by both arms; each
/// corpus is ranked by its own blended score, which are not comparable
/// across the two groups.
pub async fn semantic_search(
    state: &ServerState,
    args: &SemanticSearchArgs,
) -> Result<SemanticSearchResponse, McpError> {
    let scope = args.scope.unwrap_or_default();
    let limit = clamp_top_k_page(args.page_size);
    let query = args.query.clone();
    if query.is_empty() {
        return Err(McpError::new(
            McpErrorCode::InvalidInput,
            "semantic_search: empty query",
        ));
    }
    let want_code = matches!(scope, SearchScope::Code | SearchScope::Both);
    let want_findings = matches!(scope, SearchScope::Findings | SearchScope::Both);
    let include_external = args.include_external.unwrap_or(false);
    let include_tests = args.include_tests.unwrap_or(false);

    // Embed the query once (shared between the code and findings arms,
    // which both consume the same vector).
    let query_vec = if want_code || want_findings {
        embed_query(&query).await?
    } else {
        None
    };

    let code = if want_code {
        let query = query.clone();
        let query_vec = query_vec.clone();
        state
            .with_db(|h| async move {
                let rows = h
                    .read
                    .search_blended_hits(
                        &query,
                        query_vec.as_deref(),
                        limit,
                        include_external,
                        include_tests,
                    )
                    .await
                    .map_err(db_to_mcp)?;
                let mut items: Vec<SearchHitRef> = Vec::with_capacity(rows.len());
                for r in rows {
                    items.push(hit_to_ref(&h, r).await);
                }
                Ok(items)
            })
            .await?
    } else {
        Vec::new()
    };

    let findings = if want_findings {
        let query_vec = query_vec.clone();
        state
            .with_findings_read(|h, store| {
                Box::pin(async move {
                    let resolver = h.read.code_node_resolver().await.map_err(internal)?;
                    let hits = store
                        .search_findings(&query, query_vec.as_deref(), limit as usize, &resolver)
                        .await
                        .map_err(db_to_mcp)?;
                    Ok(hits
                        .into_iter()
                        .map(|hit| RankedFindingView {
                            finding: finding_to_view(hit.finding, hit.stale, hit.drifted),
                            score: f64::from(hit.score),
                        })
                        .collect())
                })
            })
            .await?
    } else {
        Vec::new()
    };

    Ok(SemanticSearchResponse { code, findings })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GetSourceArgs {
    /// Public id of the symbol whose source to read (e.g. `rs:foo::bar`).
    pub id: String,
}

/// Read the source text of a symbol's primary definition from disk.
pub async fn get_source(
    state: &ServerState,
    args: &GetSourceArgs,
) -> Result<SingleResponse<SourceView>, McpError> {
    if args.id.is_empty() {
        return Err(McpError::new(
            McpErrorCode::InvalidInput,
            "get_source: empty id",
        ));
    }
    let (lang, native) = split_public_id(&args.id)?;
    let source_root = state.source_root();
    state
        .with_db(|h| async move {
            let Some(row) = h.read.fetch_symbol(lang, native).await.map_err(internal)? else {
                return Ok(SingleResponse::missing(NotFoundHint::default()));
            };
            let lines = h.read.fetch_def_lines(row.id).await.map_err(internal)?;
            // Skip spurious zero-range def rows (file_id != 0 but start_line
            // == 0) so we slice from the real definition — same predicate as
            // `first_def_location_string` / `defs_for_symbol` in support.rs.
            // Otherwise `slice_lines(_, 0, _)` panics (debug) or returns
            // top-of-file text that disagrees with the reported location.
            let Some(def) = lines
                .into_iter()
                .find(|d| d.file_id != 0 && d.start_line >= 1)
            else {
                return Ok(SingleResponse::missing(NotFoundHint::default()));
            };
            let Some(path) = h
                .read
                .fetch_file_path(def.file_id)
                .await
                .map_err(internal)?
            else {
                return Ok(SingleResponse::missing(NotFoundHint::default()));
            };
            let abs = source_root.join(&path);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                return Ok(SingleResponse::missing(NotFoundHint::default()));
            };
            // Prefer the stored enclosing-item extent (whole `fn`/type body
            // incl. doc comment) when the producer supplied one — rust-analyzer
            // ≥ Dec-2025, scip-go, scip-python. When absent (older toolchain,
            // synthetic symbol), fall back to the name span: the declaration
            // line. The body span carries its own start (a doc comment sits
            // above the name), so use it for both bounds when present.
            let (start_line, end_line) =
                if def.body_end_line >= def.body_start_line && def.body_start_line >= 1 {
                    (def.body_start_line, def.body_end_line)
                } else {
                    (def.start_line, def.end_line)
                };
            let text = slice_lines(&content, start_line, end_line);
            Ok(SingleResponse::found(SourceView {
                file: path,
                start_line,
                end_line,
                text,
            }))
        })
        .await
}
