use kenn_model::Kind;
use kenn_store::api::Reader;
use kenn_store::{SymbolDocsRow, SymbolRow};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor, DecodedCursor};
use crate::error::{ConfigHint, QueryError, QueryErrorCode};
use crate::types::{
    clamp_page, clamp_top_k_page, Filters, FoundSymbolRef, GraphSummary, LanguageGraph,
    LanguageStat, ListResponse, ManagerPackages, NotFoundHint, Pagination, RankedCodeHit,
    SearchHitRef, SingleResponse, SubsetCounts, SymbolDetail, SymbolRef, WorkspaceInfo,
};
use kenn_store::StatRow;

use crate::ctx::QueryCtx;
use crate::{
    db_to_mcp, defs_for_symbol, embed_query, found_to_ref, hit_to_ref, internal, parse_language,
    split_public_id, symbol_row_to_ref,
};

#[derive(Debug, Deserialize, Default, Serialize, JsonSchema)]
pub struct GetWorkspaceOverviewArgs {}

pub async fn get_workspace_overview(
    ctx: &QueryCtx<'_>,
    _: GetWorkspaceOverviewArgs,
) -> Result<SingleResponse<WorkspaceInfo>, QueryError> {
    let config = ctx.config.clone();
    let config_present = ctx.config_present;
    // One read of the precomputed build-time stats; the overview only
    // reshapes the rows — no DB aggregation (build-time-stats).
    let rows = ctx.read.stats().await.map_err(internal)?;
    let shaped = shape_stats(rows);
    let config_hint = ConfigHint::classify(&config, shaped.symbol_count, config_present);
    Ok(SingleResponse::found(WorkspaceInfo {
        snapshot_id: ctx.snapshot_id.to_hex(),
        indexed_at: ctx.indexed_at.to_owned(),
        languages: shaped.languages,
        packages_by_manager: shaped.packages_by_manager,
        graph: shaped.graph,
        file_count: shaped.file_count,
        symbol_count: shaped.symbol_count,
        config_hint,
    }))
}

/// Reshaped `stats` rows for [`WorkspaceInfo`]. Pure data-shuffling — the
/// scalar totals are in-code sums of the subset rows, never a DB query.
struct ShapedStats {
    languages: Vec<LanguageStat>,
    packages_by_manager: Vec<ManagerPackages>,
    graph: Option<GraphSummary>,
    symbol_count: u64,
    file_count: u64,
}

#[derive(Default)]
struct LangAcc {
    symbols: SubsetCounts,
    files: SubsetCounts,
    defs: SubsetCounts,
    graph: LanguageGraph,
    has_graph: bool,
}

/// Set the subset field of `counts` named by `subset`.
fn set_subset(counts: &mut SubsetCounts, subset: &str, v: u64) {
    match subset {
        "internal" => counts.internal = v,
        "test" => counts.test = v,
        "external" => counts.external = v,
        _ => {}
    }
}

fn shape_stats(rows: Vec<StatRow>) -> ShapedStats {
    use std::collections::BTreeMap;
    let mut langs: BTreeMap<String, LangAcc> = BTreeMap::new();
    let mut mgrs: BTreeMap<String, ManagerPackages> = BTreeMap::new();
    let mut gsum = GraphSummary::default();
    let mut has_gsum = false;

    for r in rows {
        let v = u64::try_from(r.value).unwrap_or(0);
        match r.scope.as_str() {
            "language" => {
                let acc = langs.entry(r.key).or_default();
                if r.subset == "graph" {
                    acc.has_graph = true;
                    match r.metric.as_str() {
                        "nodes" => acc.graph.nodes = v,
                        "god_nodes" => acc.graph.god_nodes = v,
                        "communities" => acc.graph.communities = v,
                        "anchors" => acc.graph.anchors = v,
                        _ => {}
                    }
                } else {
                    match r.metric.as_str() {
                        "symbols" => set_subset(&mut acc.symbols, &r.subset, v),
                        "files" => set_subset(&mut acc.files, &r.subset, v),
                        "defs" => set_subset(&mut acc.defs, &r.subset, v),
                        _ => {}
                    }
                }
            }
            "manager" if r.metric == "packages" => {
                let m = mgrs
                    .entry(r.key.clone())
                    .or_insert_with(|| ManagerPackages {
                        manager: r.key.clone(),
                        internal: 0,
                        external: 0,
                    });
                match r.subset.as_str() {
                    "internal" => m.internal = v,
                    "external" => m.external = v,
                    _ => {}
                }
            }
            "global" if r.subset == "graph" => {
                has_gsum = true;
                match r.metric.as_str() {
                    "hierarchy_depth" => gsum.hierarchy_depth = v,
                    "cross_anchor_communities" => gsum.cross_anchor_communities = v,
                    "domains" => gsum.domains = v,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let (mut symbol_count, mut file_count) = (0u64, 0u64);
    let mut languages = Vec::new();
    for (key, acc) in langs {
        let Some(language) = parse_language(&key) else {
            continue;
        };
        symbol_count += acc.symbols.internal + acc.symbols.test + acc.symbols.external;
        file_count += acc.files.internal + acc.files.test + acc.files.external;
        languages.push(LanguageStat {
            language,
            symbols: acc.symbols,
            files: acc.files,
            defs: acc.defs,
            graph: acc.has_graph.then_some(acc.graph),
        });
    }

    ShapedStats {
        languages,
        packages_by_manager: mgrs.into_values().collect(),
        graph: has_gsum.then_some(gsum),
        symbol_count,
        file_count,
    }
}

// ── SEARCH ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GetSymbolArgs {
    pub id: String,
}

pub async fn get_symbol(
    ctx: &QueryCtx<'_>,
    args: &GetSymbolArgs,
) -> Result<SingleResponse<SymbolDetail>, QueryError> {
    if args.id.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "get_symbol: empty id",
        ));
    }
    let (lang, native) = split_public_id(&args.id)?;
    let row = ctx
        .read
        .fetch_symbol(lang, native)
        .await
        .map_err(internal)?;
    let Some(row) = row else {
        return Ok(SingleResponse::missing(NotFoundHint::default()));
    };
    let docs = ctx
        .read
        .fetch_symbol_docs_row(row.id)
        .await
        .map_err(internal)?;
    let parent = if row.enclosing_sym_id == 0 {
        None
    } else {
        let p = ctx
            .read
            .fetch_symbol_by_short_id(row.enclosing_sym_id)
            .await
            .map_err(internal)?;
        if let Some(p) = p {
            Some(symbol_row_to_ref(ctx.read, &p, None, None).await)
        } else {
            None
        }
    };
    let defs = defs_for_symbol(ctx.read, row.id).await;
    let base = symbol_row_to_ref(ctx.read, &row, None, None).await;
    let docs = docs.unwrap_or(SymbolDocsRow {
        sig: String::new(),
        doc: String::new(),
    });
    Ok(SingleResponse::found(SymbolDetail {
        base,
        sig: docs.sig,
        doc: docs.doc,
        defined_in: parent,
        defs,
    }))
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct FindSimilarArgs {
    /// Public id of the symbol to find neighbours of — the `id` field
    /// returned by `search_symbols` / `find_symbol` / `get_symbol`
    /// (e.g. `cs:Models.Order`).
    pub id: String,
    /// Rows per response.
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub include_tests: Option<bool>,
    #[serde(default)]
    pub include_external: Option<bool>,
}

/// Item-to-item semantic search: symbols whose embedding is nearest the
/// given symbol's own committed vector. Surfaces related code the name
/// and relationship tools miss — parallel implementations across
/// subprojects, look-alike logic with no shared call edge. Reuses the
/// committed vector, so it needs no embedding model — but errors with
/// `EmbeddingUnavailable` when the symbol has no committed vector (vectors
/// not built), so an agent does not mistake "not embedded" for "no matches."
pub async fn find_similar(
    ctx: &QueryCtx<'_>,
    args: &FindSimilarArgs,
) -> Result<ListResponse<RankedCodeHit>, QueryError> {
    if args.id.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "find_similar: empty id",
        ));
    }
    let (lang, native) = split_public_id(&args.id)?;
    let limit = clamp_page(args.page_size);
    let include_external = args.include_external.unwrap_or(false);
    let include_tests = args.include_tests.unwrap_or(false);
    // Captured (Copy) into the closure to make a missing vector transient while
    // the background embed pass is still building, terminal once it has settled.
    let embed_stage = ctx.embed_stage;
    let Some(source) = ctx
        .read
        .fetch_symbol(lang, native)
        .await
        .map_err(internal)?
    else {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!("find_similar: no symbol with id `{native}` in the current index"),
        ));
    };
    let Some(rows) = ctx
        .read
        .find_similar_symbols(source.id, limit, include_external, include_tests)
        .await
        .map_err(internal)?
    else {
        // No committed vector for this symbol — distinct from "no
        // similar found." Transient while the embed pass is still
        // building (retry), terminal once it has settled.
        return Err(match embed_stage {
            crate::types::EmbedStage::Building => QueryError::new(
                QueryErrorCode::EmbedderStarting,
                format!(
                    "find_similar: symbol `{native}` has no committed embedding yet — the \
                             embedding pass is still building (get_index_status: state=embedding); \
                             retry shortly"
                ),
            ),
            _ => QueryError::new(
                QueryErrorCode::EmbeddingUnavailable,
                format!(
                    "find_similar: symbol `{native}` has no committed embedding — run \
                             `kenn embed` to build vectors, or this symbol has no embeddable text"
                ),
            ),
        });
    };
    let mut items: Vec<RankedCodeHit> = Vec::with_capacity(rows.len());
    for r in rows {
        let score = r.score;
        let symbol: SymbolRow = r.into();
        items.push(RankedCodeHit {
            symbol: symbol_row_to_ref(ctx.read, &symbol, None, None).await,
            score,
        });
    }
    Ok(ListResponse { items, next: None })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SearchSymbolsArgs {
    pub query: String,
    #[serde(default)]
    pub filters: Option<Filters>,
    /// Optional `page_size` and continuation cursor. Pages are
    /// ordered by the blended score; see `mcp-symbol-search`.
    #[serde(default)]
    pub pagination: Option<Pagination>,
}

/// BM25 over name AND docstrings, blended (`3·name + 1·doc + 5·subs`)
/// and ordered by `(score DESC, len(name) ASC, short_id ASC)`. Use
/// when the agent has natural-language intent. For literal-name
/// lookup use [`find_symbol`] instead.
pub async fn search_symbols(
    ctx: &QueryCtx<'_>,
    args: &SearchSymbolsArgs,
) -> Result<ListResponse<SearchHitRef>, QueryError> {
    use crate::cursor::encode_topk_cursor;
    use crate::types::TOP_K_MATERIALIZE;

    let page_size = clamp_top_k_page(args.pagination.as_ref().and_then(|p| p.page_size)) as usize;
    let filters = args.filters.clone().unwrap_or_default();
    let include_external = filters.include_external.unwrap_or(false);
    let include_tests = filters.include_tests.unwrap_or(false);
    let cursor = if let Some(c) = args.pagination.as_ref().and_then(|p| p.cursor.as_ref()) {
        Some(decode_cursor(c)?)
    } else {
        None
    };
    let query = args.query.clone();

    // Continuation: serve from the cache, never touch the reader.
    // Cache stores already-converted RankedSymbolRef — no async hydration.
    if let Some(DecodedCursor::TopK { cache_id, offset }) = cursor {
        let (items, total) =
            ctx.caches
                .symbols
                .slice(cache_id, offset, page_size, ctx.snapshot_id)?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "items.len() ≤ page_size ≤ MAX_TOP_K_PAGE = 30"
        )]
        let new_offset = offset + items.len() as u32;
        let next = if (new_offset as usize) < total {
            Some(encode_topk_cursor(cache_id, new_offset))
        } else {
            None
        };
        return Ok(ListResponse { items, next });
    }
    // Any non-TopK cursor here is wrong shape for this tool.
    if cursor.is_some() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "search_symbols: cursor is not a top-K cursor",
        ));
    }

    // Embed the query once, outside the reader closure — the store
    // accepts the resolved vector and stays free of `kenn_embed`.
    let query_vec = embed_query(&query).await?;
    let rows = ctx
        .read
        .search_blended_hits(
            &query,
            query_vec.as_deref(),
            TOP_K_MATERIALIZE,
            include_external,
            include_tests,
        )
        .await
        .map_err(db_to_mcp)?;
    // Convert to the wire row type (does an async per-row hydration of refs).
    let mut all_items: Vec<SearchHitRef> = Vec::with_capacity(rows.len());
    for r in rows {
        all_items.push(hit_to_ref(ctx.read, r).await);
    }
    // Single-shot if everything fits in one response.
    if all_items.len() <= page_size {
        return Ok(ListResponse {
            items: all_items,
            next: None,
        });
    }
    // Multi-page: stash, take first slice, emit cursor.
    let (cache_id, first_page) =
        ctx.caches
            .symbols
            .put_and_take_first_page(ctx.snapshot_id, all_items, page_size);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page_size ≤ MAX_TOP_K_PAGE = 30"
    )]
    let next_offset = page_size as u32;
    let next = Some(encode_topk_cursor(cache_id, next_offset));
    Ok(ListResponse {
        items: first_page,
        next,
    })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct FindSymbolArgs {
    /// The literal name to search for (e.g. `OrderHandler`,
    /// `Models.Order`, `IHttpClientFactory`). Case-insensitive.
    pub name: String,
    /// Optional `Kind` filter applied after the tiered match.
    #[serde(default)]
    pub kind: Option<Vec<Kind>>,
    /// Rows per response.
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub include_tests: Option<bool>,
    #[serde(default)]
    pub include_external: Option<bool>,
}

/// 4-tier name lookup: exact → prefix range → case-insensitive
/// substring → n-gram fuzzy. Each row carries `match_kind`
/// so the agent knows the strength of the match. Use when the agent
/// has a literal name; for natural-language search use
/// [`search_symbols`].
pub async fn find_symbol(
    ctx: &QueryCtx<'_>,
    args: &FindSymbolArgs,
) -> Result<ListResponse<FoundSymbolRef>, QueryError> {
    if args.name.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "find_symbol: empty name",
        ));
    }
    let limit = clamp_page(args.page_size);
    let include_external = args.include_external.unwrap_or(false);
    let include_tests = args.include_tests.unwrap_or(false);
    let name = args.name.clone();
    let kind_filter = args.kind.clone();
    let mut hits = ctx
        .read
        .find_symbol_tiered(&name, limit, include_external, include_tests)
        .await
        .map_err(internal)?;
    if let Some(kinds) = kind_filter {
        let allowed: Vec<&'static str> = kinds.iter().map(|k| k.db_name()).collect();
        hits.retain(|ctx| allowed.iter().any(|k| *k == ctx.symbol.kind));
    }
    let mut items: Vec<FoundSymbolRef> = Vec::with_capacity(hits.len());
    for h_row in hits {
        items.push(found_to_ref(ctx.read, h_row).await);
    }
    Ok(ListResponse { items, next: None })
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct FindAtLocationArgs {
    /// File to look in — a workspace-relative or absolute path.
    pub file_path: String,
    /// 1-based line number — paste it straight from a stack trace,
    /// editor "go to line", or a prior `get_source` / wire `#<line>`.
    pub line: u32,
    #[serde(default)]
    pub kind: Option<Vec<Kind>>,
}

pub async fn find_at_location(
    ctx: &QueryCtx<'_>,
    args: &FindAtLocationArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    if args.file_path.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "find_at_location: empty file_path",
        ));
    }
    let file_path = args.file_path.clone();
    let line = args.line;
    let kind = args.kind.clone();
    // The named file must be in the current index — a path the
    // agent gave but the index cannot see is a mistake worth
    // surfacing, not a silently empty result.
    let Some(file_id) = ctx
        .read
        .fetch_file_short_id(&file_path)
        .await
        .map_err(internal)?
    else {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!("find_at_location: file not in the current index: {file_path}"),
        ));
    };
    let mut rows = ctx
        .read
        .find_at_location(file_id, line)
        .await
        .map_err(internal)?;
    if let Some(kinds) = &kind {
        let allowed: Vec<&'static str> = kinds.iter().map(|k| k.db_name()).collect();
        rows.retain(|r| allowed.iter().any(|k| *k == r.kind));
    }
    let mut items: Vec<SymbolRef> = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(symbol_row_to_ref(ctx.read, &r, None, None).await);
    }
    Ok(ListResponse { items, next: None })
}

#[cfg(test)]
mod stats_shape_tests {
    use super::{shape_stats, StatRow};

    fn row(scope: &str, key: &str, subset: &str, metric: &str, value: i64) -> StatRow {
        StatRow {
            scope: scope.into(),
            key: key.into(),
            subset: subset.into(),
            metric: metric.into(),
            value,
        }
    }

    #[test]
    fn reshapes_language_manager_and_graph_rows() {
        let rows = vec![
            row("language", "rust", "internal", "symbols", 3),
            row("language", "rust", "test", "symbols", 1),
            row("language", "rust", "external", "symbols", 2),
            row("language", "rust", "internal", "files", 4),
            row("language", "rust", "internal", "defs", 5),
            row("language", "rust", "graph", "nodes", 9),
            row("language", "rust", "graph", "god_nodes", 2),
            row("language", "rust", "graph", "communities", 7),
            row("language", "rust", "graph", "anchors", 3),
            // an unknown language key is skipped by parse_language
            row("language", "klingon", "internal", "symbols", 99),
            // unknown metric/subset hit the fall-through arms
            row("language", "rust", "internal", "weird", 1),
            row("language", "rust", "weird", "symbols", 1),
            row("manager", "cargo", "internal", "packages", 1),
            row("manager", "cargo", "external", "packages", 8),
            row("global", "", "graph", "hierarchy_depth", 5),
            // The two community counters are DIFFERENT questions and routinely
            // differ several-fold; the pair is the whole point (38 raw vs 9
            // earned on this repo, unlabelled, was the reported bug).
            row("global", "", "graph", "cross_anchor_communities", 6),
            row("global", "", "graph", "domains", 2),
            row("global", "", "graph", "weird", 1),
        ];
        let s = shape_stats(rows);

        assert_eq!(s.symbol_count, 6); // 3 + 1 + 2; klingon excluded
        assert_eq!(s.file_count, 4);
        assert_eq!(s.languages.len(), 1);
        let rust = &s.languages[0];
        assert_eq!(rust.symbols.internal, 3);
        assert_eq!(rust.symbols.test, 1);
        assert_eq!(rust.symbols.external, 2);
        assert_eq!(rust.files.internal, 4);
        assert_eq!(rust.defs.internal, 5);
        let g = rust.graph.as_ref().expect("graph counters present");
        assert_eq!(
            (g.nodes, g.god_nodes, g.communities, g.anchors),
            (9, 2, 7, 3)
        );

        assert_eq!(s.packages_by_manager.len(), 1);
        assert_eq!(s.packages_by_manager[0].manager, "cargo");
        assert_eq!(s.packages_by_manager[0].internal, 1);
        assert_eq!(s.packages_by_manager[0].external, 8);

        let graph = s.graph.expect("whole-graph summary present");
        assert_eq!(graph.hierarchy_depth, 5);
        // Both counters are reported, each carrying its own row's value — the
        // earned count must never be filled from the raw one (or vice versa).
        // Mutation-checked: dropping the `"domains"` arm from the reshaping match
        // leaves this 0.
        assert_eq!(graph.cross_anchor_communities, 6);
        assert_eq!(graph.domains, 2);
    }

    #[test]
    fn no_graph_rows_means_no_graph_block() {
        let rows = vec![row("language", "rust", "internal", "symbols", 1)];
        let s = shape_stats(rows);
        assert!(s.graph.is_none());
        assert!(s.languages[0].graph.is_none());
    }
}
