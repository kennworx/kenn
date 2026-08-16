//! Findings store tools: get/search/store/merge + the predecessor /
//! successor DAG traversal.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor, DecodedCursor};
use crate::error::{QueryError, QueryErrorCode};
use crate::types::{
    clamp_top_k_page, FindingView, ListResponse, NotFoundHint, Pagination, RankedFindingView,
    SingleResponse, StoreFindingResponse,
};

use crate::ctx::QueryCtx;
use crate::{db_to_mcp, embed_query, finding_to_view, internal};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GetFindingArgs {
    /// The finding id (`fnd_…`).
    pub id: String,
}

/// Fetch a finding by id. Returns the raw record regardless of
/// supersede / tombstone state.
pub async fn get_finding(
    ctx: &QueryCtx<'_>,
    args: &GetFindingArgs,
) -> Result<SingleResponse<FindingView>, QueryError> {
    if args.id.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "get_finding: empty id",
        ));
    }
    let id = args.id.clone();
    let store = ctx.findings_read().await?;
    let found = store.get_finding(&id).await.map_err(internal)?;
    Ok(match found {
        Some(f) => SingleResponse::found(finding_to_view(f, false, false)),
        None => SingleResponse::missing(NotFoundHint::default()),
    })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SearchFindingsArgs {
    pub query: String,
    /// Optional `page_size` and continuation cursor.
    #[serde(default)]
    pub pagination: Option<Pagination>,
}

/// BM25 search over findings. Superseded / tombstoned findings are
/// excluded; each surviving hit carries a `stale` flag.
pub async fn search_findings(
    ctx: &QueryCtx<'_>,
    args: &SearchFindingsArgs,
) -> Result<ListResponse<RankedFindingView>, QueryError> {
    use crate::cursor::encode_topk_cursor;
    use crate::types::TOP_K_MATERIALIZE;

    let page_size = clamp_top_k_page(args.pagination.as_ref().and_then(|p| p.page_size)) as usize;
    let cursor = if let Some(c) = args.pagination.as_ref().and_then(|p| p.cursor.as_ref()) {
        Some(decode_cursor(c)?)
    } else {
        None
    };
    let query = args.query.clone();
    if query.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "search_findings: empty query",
        ));
    }

    // Continuation path: serve from the cache.
    if let Some(DecodedCursor::TopK { cache_id, offset }) = cursor {
        let (rows, total) =
            ctx.caches
                .findings
                .slice(cache_id, offset, page_size, ctx.snapshot_id)?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rows.len() ≤ page_size ≤ MAX_TOP_K_PAGE = 30"
        )]
        let new_offset = offset + rows.len() as u32;
        let next = if (new_offset as usize) < total {
            Some(encode_topk_cursor(cache_id, new_offset))
        } else {
            None
        };
        return Ok(ListResponse { items: rows, next });
    }
    if cursor.is_some() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "search_findings: cursor is not a top-K cursor",
        ));
    }

    // First call: materialize the top-K window, then stash it in the cache.
    //
    // This used to be split around a closure, because `with_findings_read`'s
    // future was `'static` and so could not capture `state` by reference —
    // the snapshot id had to be fetched in a separate `with_db` first, and the
    // cache write had to happen after the closure returned. Holding the guard
    // in this scope removes both constraints: it is one straight sequence now.
    let query_vec = embed_query(&query).await?;
    let all_items: Vec<RankedFindingView> = {
        let store = ctx.findings_read().await?;
        let resolver = ctx.read.code_node_resolver().await.map_err(internal)?;
        let hits = store
            .search_findings(
                &query,
                query_vec.as_deref(),
                TOP_K_MATERIALIZE as usize,
                &resolver,
            )
            .await
            .map_err(db_to_mcp)?;
        hits.into_iter()
            .map(|hit| RankedFindingView {
                finding: finding_to_view(hit.finding, hit.stale, hit.drifted),
                score: f64::from(hit.score),
            })
            .collect()
    };
    if all_items.len() <= page_size {
        return Ok(ListResponse {
            items: all_items,
            next: None,
        });
    }
    let (cache_id, first_page) =
        ctx.caches
            .findings
            .put_and_take_first_page(ctx.snapshot_id, all_items, page_size);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page_size ≤ MAX_TOP_K_PAGE = 30"
    )]
    let next_offset = page_size as u32;
    Ok(ListResponse {
        items: first_page,
        next: Some(encode_topk_cursor(cache_id, next_offset)),
    })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct StoreFindingArgs {
    /// Free-form prose payload — what the finding states.
    pub text: String,
    /// Provenance edges — code-graph node ids and/or earlier finding
    /// ids this finding was derived from.
    #[serde(default)]
    pub parent_ids: Option<Vec<String>>,
    /// Free-form classification labels.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// File/dir paths this finding applies to (anchors). Each is recorded as an
    /// initial `attach` so the finding is created and anchored in one call —
    /// useful for a directive (`tags: ["directive", "polarity:dont"]`).
    #[serde(default)]
    pub anchors: Option<Vec<String>>,
}

/// Store a finding and flush it to the durable store — the finding is
/// committed when this returns. `similar` carries any committed
/// findings whose content is semantically near-duplicate of the new
/// text (empty when no embedding model is available).
pub async fn store_finding(
    ctx: &QueryCtx<'_>,
    args: &StoreFindingArgs,
) -> Result<StoreFindingResponse, QueryError> {
    if args.text.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "store_finding: empty text",
        ));
    }
    let text = args.text.clone();
    let parent_ids = args.parent_ids.clone().unwrap_or_default();
    let tags = args.tags.clone().unwrap_or_default();
    let anchor_ts = kenn_store::Timestamp::now();
    // Hash each file anchor against the live working tree at attach time so
    // later content drift can be detected; a directory/unreadable path → None.
    let source_root = ctx.source_root.clone();
    let anchors: Vec<(String, Option<String>)> = args
        .anchors
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            let sha = kenn_store::file_content_sha(&source_root.join(&a));
            (a, sha)
        })
        .collect();
    // Pre-embed the text for the near-duplicate probe — the store no
    // longer reaches into the embedder itself. The probe is advisory, so a
    // cold embedder degrades to skipping it rather than failing the write
    // (matching `find_directives`' semantic leg) — otherwise the first
    // `store_finding` against a freshly-indexed repo errors with
    // `EmbedderStarting` and the finding is lost.
    let text_vec = match embed_query(&text).await {
        Ok(v) => v,
        Err(e) if e.code == QueryErrorCode::EmbedderStarting => None,
        Err(e) => return Err(e),
    };
    let mut store = ctx.findings_write().await?;
    // Each `fnd_…` parent must name a real finding; code-node
    // references are accepted as-is (best-effort provenance).
    // Collect every unknown finding parent so the caller
    // fixes them in one round-trip.
    let mut missing: Vec<String> = Vec::new();
    for pid in &parent_ids {
        if !dag_id_exists(&store, pid).await? {
            missing.push(format!("`{pid}`"));
        }
    }
    if !missing.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!(
                "store_finding: unknown parent id(s): {}",
                missing.join(", ")
            ),
        ));
    }
    let (id, similar) = store
        .store_finding(text, parent_ids, tags, text_vec.as_deref())
        .await
        .map_err(internal)?;
    // Flush the record FIRST, then record anchors — so a failed
    // flush never leaves an orphan `<id>.anchor.jsonl` for a finding
    // that was never committed.
    store.flush().await.map_err(internal)?;
    for (anchor, sha) in anchors {
        store
            .record_anchor_event(
                &id,
                &kenn_store::AnchorEvent::Attach {
                    anchor,
                    ts: anchor_ts,
                    sha,
                },
            )
            .await
            .map_err(internal)?;
    }
    Ok(StoreFindingResponse {
        id,
        similar: similar
            .into_iter()
            .map(|f| finding_to_view(f, false, false))
            .collect(),
    })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct MergeFindingsArgs {
    /// The findings to synthesize from — recorded as the new finding's
    /// `parent_ids`.
    pub ids: Vec<String>,
    /// Prose payload of the synthesized finding.
    pub text: String,
    /// Free-form classification labels.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Synthesize a new finding from several inputs — stores a new finding
/// whose `parent_ids` are `ids` — and flush. Returns the new id.
pub async fn merge_findings(
    ctx: &QueryCtx<'_>,
    args: &MergeFindingsArgs,
) -> Result<SingleResponse<String>, QueryError> {
    if args.ids.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "merge_findings: empty ids",
        ));
    }
    if args.text.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "merge_findings: empty text",
        ));
    }
    let ids = args.ids.clone();
    let text = args.text.clone();
    let tags = args.tags.clone().unwrap_or_default();
    let mut store = ctx.findings_write().await?;
    let mut missing: Vec<String> = Vec::new();
    for fid in &ids {
        if store.get_finding(fid).await.map_err(internal)?.is_none() {
            missing.push(format!("`{fid}`"));
        }
    }
    if !missing.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!(
                "merge_findings: unknown finding id(s): {}",
                missing.join(", ")
            ),
        ));
    }
    let id = store.merge_findings(ids, text, tags);
    store.flush().await.map_err(internal)?;
    Ok(SingleResponse::found(id))
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct FindingDagArgs {
    /// The finding (or code-node) id to walk from.
    pub id: String,
}

/// Whether a finding-DAG id resolves.
///
/// A `fnd_…` id must name a real finding. Any other id is a code-graph
/// node reference — deliberately loose: findings are durable, and a
/// code node a finding cites may later be refactored away (which
/// `finding_is_stale` surfaces). Code-node ids are therefore accepted
/// as-is, never validated against the current snapshot — the finding-DAG
/// id space (`<lang>:<pub_id>`) is not the symbol-search `pub_id` space
/// anyway.
async fn dag_id_exists(store: &kenn_store::FindingsStore, id: &str) -> Result<bool, QueryError> {
    // Finding ids carry the `fnd_` prefix (kenn-store FINDING_ID_PREFIX).
    if id.starts_with("fnd_") {
        return Ok(store.get_finding(id).await.map_err(internal)?.is_some());
    }
    Ok(true)
}

/// Verify a finding-DAG start id resolves; `InvalidInput` otherwise.
/// Only an unknown `fnd_…` id is rejected — see [`dag_id_exists`].
async fn ensure_dag_id_exists(
    store: &kenn_store::FindingsStore,
    id: &str,
    tool: &str,
) -> Result<(), QueryError> {
    if dag_id_exists(store, id).await? {
        Ok(())
    } else {
        Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!("{tool}: no finding with id `{id}`"),
        ))
    }
}

/// Transitively collect every id reachable from `id` through
/// `parent_ids` — the derivation provenance, walkable to code nodes.
pub async fn find_predecessors(
    ctx: &QueryCtx<'_>,
    args: &FindingDagArgs,
) -> Result<ListResponse<String>, QueryError> {
    if args.id.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "find_predecessors: empty id",
        ));
    }
    let id = args.id.clone();
    let store = ctx.findings_read().await?;
    ensure_dag_id_exists(&store, &id, "find_predecessors").await?;
    let ids = store.find_predecessors(&id).await.map_err(internal)?;
    Ok(ListResponse {
        items: ids,
        next: None,
    })
}

/// Transitively collect every finding id that derives from `id`.
pub async fn find_successors(
    ctx: &QueryCtx<'_>,
    args: &FindingDagArgs,
) -> Result<ListResponse<String>, QueryError> {
    if args.id.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "find_successors: empty id",
        ));
    }
    let id = args.id.clone();
    let store = ctx.findings_read().await?;
    ensure_dag_id_exists(&store, &id, "find_successors").await?;
    let ids = store.find_successors(&id).await.map_err(internal)?;
    Ok(ListResponse {
        items: ids,
        next: None,
    })
}
