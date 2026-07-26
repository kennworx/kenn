use kenn_model::{EdgeKind, Kind, Language};
use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor, encode_usages_cursor, DecodedCursor};
use crate::error::{McpError, McpErrorCode};
use crate::types::{clamp_page, FindUsagesResponse, SymbolRef, UsageRef};

use super::super::{
    ensure_cursor_matches, internal, split_public_id, symbol_row_to_ref, ReadyView, ServerState,
};

// ── FIND USAGES  (resolution + inbound traversal, fused) ────────────────────

/// Max distinct resolved targets a multi-target `find_usages` answer
/// covers. Beyond this the target set is trimmed and `truncated` is set;
/// the caller narrows (filter or `pub_id`) to a single, paginating target.
const FIND_USAGES_TARGET_CAP: usize = 10;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FindUsagesArgs {
    /// A name, a workspace-relative path, or a `pub_id`. A `pub_id`
    /// (e.g. `cs:Models.Order`) is used directly; a path
    /// (`src/orders/api.ts`, `assets/logo.png`) resolves via the file
    /// lookup; anything else goes through the name index.
    pub query: String,
    /// Narrow an ambiguous name to one target by symbol kind.
    #[serde(default)]
    pub kind: Option<Vec<Kind>>,
    /// Narrow by the candidate's definition file (workspace-relative).
    #[serde(default)]
    pub path: Option<Vec<String>>,
    /// Narrow by resolving package name.
    #[serde(default)]
    pub package: Option<Vec<String>>,
    /// Narrow by language.
    #[serde(default)]
    pub language: Option<Vec<Language>>,
    /// Override the default reference-style edge set.
    #[serde(default)]
    pub edge_kinds: Option<Vec<EdgeKind>>,
    #[serde(default)]
    pub include_external: Option<bool>,
    #[serde(default)]
    pub include_tests: Option<bool>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// `find_usages` default edge selection — the reference-style edges, so
/// "where used" spans calls, type/field references, module imports,
/// document links, and class usage. `imports` is load-bearing: a
/// file/stylesheet target's `<link>`/`<script>`/module importers are
/// `imports` edges.
fn default_usages_edge_kinds() -> Vec<EdgeKind> {
    vec![
        EdgeKind::Calls,
        EdgeKind::TypeUse,
        EdgeKind::FieldAccess,
        EdgeKind::Instantiates,
        EdgeKind::Imports,
        EdgeKind::LinksTo,
        EdgeKind::LinksToFile,
        EdgeKind::Embeds,
        EdgeKind::UsesCssClass,
    ]
}

/// A resolved `find_usages` target: the node's within-snapshot id (the
/// traversal pivot) plus a human-facing label (`pub_id` or file path)
/// each reference is tagged with.
struct ResolvedTarget {
    short_id: u32,
    label: String,
}

/// The candidate's definition path (location minus the `./` prefix and
/// the `#line` suffix), used by the `path` narrowing filter.
fn ref_def_path(r: &SymbolRef) -> Option<String> {
    let loc = r.location.as_ref()?;
    let no_hash = loc.split('#').next().unwrap_or(loc);
    Some(no_hash.strip_prefix("./").unwrap_or(no_hash).to_owned())
}

/// True when a name-resolved candidate survives the optional
/// `kind`/`language`/`package`/`path` narrowing filters (AND across
/// filter kinds, OR within each list).
fn passes_narrowing(r: &SymbolRef, args: &FindUsagesArgs) -> bool {
    if let Some(kinds) = &args.kind {
        if !kinds.contains(&r.kind) {
            return false;
        }
    }
    if let Some(langs) = &args.language {
        if !langs.contains(&r.language) {
            return false;
        }
    }
    if let Some(pkgs) = &args.package {
        if !pkgs.contains(&r.package) {
            return false;
        }
    }
    if let Some(paths) = &args.path {
        let cand = ref_def_path(r);
        if !paths
            .iter()
            .any(|p| cand.as_deref() == Some(p.strip_prefix("./").unwrap_or(p)))
        {
            return false;
        }
    }
    true
}

/// Resolution dispatch (design D2): a `pub_id` that resolves is one
/// target; otherwise a path that resolves to a file node is one target;
/// otherwise the name index yields N candidates, narrowed by the
/// filters. An unresolved query yields an empty vec — the search-tool
/// exemption (D4), surfaced as an empty response, never an error.
async fn resolve_targets(
    h: &ReadyView,
    args: &FindUsagesArgs,
    limit: u32,
) -> Result<Vec<ResolvedTarget>, McpError> {
    let q = &args.query;
    // 1. pub_id used directly (only when it actually resolves).
    if let Ok((lang, native)) = split_public_id(q) {
        if let Some(row) = h.read.fetch_symbol(lang, native).await.map_err(internal)? {
            return Ok(vec![ResolvedTarget {
                short_id: row.id,
                label: row.pub_id,
            }]);
        }
    }
    // 2. Workspace-relative path → the file node (NOT the name index).
    if let Some(file_id) = h.read.fetch_file_short_id(q).await.map_err(internal)? {
        return Ok(vec![ResolvedTarget {
            short_id: file_id,
            label: q.clone(),
        }]);
    }
    // 3. Plain name → the name index. Resolution always considers external
    // AND test nodes so an asset's attachment stub (`external = true`, e.g.
    // `![[logo.png]]`), external symbols, and test-defined symbols are all
    // reachable as targets — you can ask for the usages of a test helper by
    // name. `args.include_external` / `args.include_tests` instead govern
    // which *referencing* nodes the traversal returns.
    let hits = h
        .read
        .find_symbol_tiered(q, limit, true, true)
        .await
        .map_err(internal)?;
    let mut targets = Vec::new();
    for hit in hits {
        let row = hit.symbol;
        let r = symbol_row_to_ref(h, &row, None, None).await;
        if passes_narrowing(&r, args) {
            targets.push(ResolvedTarget {
                short_id: row.id,
                label: row.pub_id,
            });
        }
    }
    Ok(targets)
}

/// Walk one fixed target's incoming references across `edges` in order,
/// producing one page plus the `next` cursor. The cursor is an
/// `(edge_ordinal, last_short_id)` pair: pages within a kind via
/// `list_inbound`, and when a kind is exhausted the ordinal advances and
/// `last_short_id` resets to 0.
async fn paginate_single_target(
    h: &ReadyView,
    target: &ResolvedTarget,
    edges: &[EdgeKind],
    start: Option<(u8, u32)>,
    limit: u32,
    include_external: bool,
    include_tests: bool,
) -> Result<(Vec<UsageRef>, Option<String>), McpError> {
    let mut items: Vec<UsageRef> = Vec::new();
    let (mut ordinal, mut after) = start.unwrap_or((0, 0));
    let mut next: Option<String> = None;
    loop {
        let oi = ordinal as usize;
        let Some(&edge) = edges.get(oi) else {
            break;
        };
        let done = u32::try_from(items.len()).unwrap_or(u32::MAX);
        let remaining = limit.saturating_sub(done);
        if remaining == 0 {
            next = Some(encode_usages_cursor(h.snapshot_id, ordinal, after));
            break;
        }
        let cursor_after = (after != 0).then_some(after);
        let (rows, total) = h
            .read
            .list_inbound(
                target.short_id,
                edge.db_name(),
                remaining,
                cursor_after,
                &kenn_store::RowNarrow::visibility(include_external, include_tests),
            )
            .await
            .map_err(internal)?;
        let returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        for r in &rows {
            after = r.id;
            let reference = symbol_row_to_ref(h, r, Some(edge), None).await;
            items.push(UsageRef {
                reference,
                target: target.label.clone(),
            });
        }
        if returned == remaining {
            // Page is full at this kind's boundary — emit a cursor that
            // resumes either mid-kind or at the next kind.
            if u64::from(returned) < total {
                next = Some(encode_usages_cursor(h.snapshot_id, ordinal, after));
            } else if oi + 1 < edges.len() {
                let no = u8::try_from(oi + 1).unwrap_or(u8::MAX);
                next = Some(encode_usages_cursor(h.snapshot_id, no, 0));
            }
            break;
        }
        // Kind exhausted before filling the page — advance to the next.
        ordinal = ordinal.saturating_add(1);
        after = 0;
    }
    Ok((items, next))
}

/// Collect references across several targets × edges into one flat,
/// target-tagged list, capped at `limit` rows total. Used for the
/// multi-target (ambiguous) case, which does not paginate.
async fn collect_multi_target(
    h: &ReadyView,
    targets: &[ResolvedTarget],
    edges: &[EdgeKind],
    limit: usize,
    include_external: bool,
    include_tests: bool,
) -> Result<Vec<UsageRef>, McpError> {
    let mut items: Vec<UsageRef> = Vec::new();
    'outer: for t in targets {
        for &edge in edges {
            if items.len() >= limit {
                break 'outer;
            }
            let remaining = u32::try_from(limit - items.len()).unwrap_or(u32::MAX);
            let (rows, _total) = h
                .read
                .list_inbound(
                    t.short_id,
                    edge.db_name(),
                    remaining,
                    None,
                    &kenn_store::RowNarrow::visibility(include_external, include_tests),
                )
                .await
                .map_err(internal)?;
            for r in &rows {
                let reference = symbol_row_to_ref(h, r, Some(edge), None).await;
                items.push(UsageRef {
                    reference,
                    target: t.label.clone(),
                });
                if items.len() >= limit {
                    break;
                }
            }
        }
    }
    Ok(items)
}

/// One call from a name / path / `pub_id` to its incoming references —
/// the `find_symbol` + `list_usages` join, server-side. Resolution
/// dispatches on query form (D2); the default edge set is reference-style
/// (D3); an unresolved or unreferenced query returns empty, not an error
/// (D4). Pagination is single-target only: exactly one resolved target
/// paginates with a `next` cursor; more than one returns the capped flat
/// tagged list with `next: null` and `truncated` set.
pub async fn find_usages(
    state: &ServerState,
    args: &FindUsagesArgs,
) -> Result<FindUsagesResponse, McpError> {
    if args.query.trim().is_empty() {
        return Err(McpError::new(
            McpErrorCode::InvalidInput,
            "find_usages: empty query",
        ));
    }
    let limit = clamp_page(args.page_size);
    let include_external = args.include_external.unwrap_or(false);
    // Universal default: exclude test symbols unless the caller opts in
    // (`include_tests = true`). Matches the navigation and search tools. Note:
    // narrowing a refactor's reference scope to non-test sites is deliberate —
    // pass `include_tests: true` to include test call sites.
    let include_tests = args.include_tests.unwrap_or(false);
    let edges = args
        .edge_kinds
        .clone()
        .unwrap_or_else(default_usages_edge_kinds);
    let decoded = match &args.cursor {
        Some(c) => Some(decode_cursor(c)?),
        None => None,
    };
    if let Some(d) = &decoded {
        if !matches!(d, DecodedCursor::Usages { .. }) {
            return Err(McpError::new(
                McpErrorCode::InvalidInput,
                "find_usages: cursor is not a find_usages cursor",
            ));
        }
    }
    let args = args.clone();
    state
        .with_db(|h| async move {
            if let Some(d) = &decoded {
                ensure_cursor_matches(&h, d)?;
            }
            let targets = resolve_targets(&h, &args, limit).await?;
            // Search-tool exemption (D4): nothing matched → empty, not an error.
            if targets.is_empty() {
                return Ok(FindUsagesResponse::default());
            }
            if let [target] = targets.as_slice() {
                let start = match decoded {
                    Some(DecodedCursor::Usages {
                        edge_ordinal,
                        last_short_id,
                        ..
                    }) => Some((edge_ordinal, last_short_id)),
                    _ => None,
                };
                let (items, next) = paginate_single_target(
                    &h,
                    target,
                    &edges,
                    start,
                    limit,
                    include_external,
                    include_tests,
                )
                .await?;
                return Ok(FindUsagesResponse {
                    items,
                    next,
                    targets: 1,
                    truncated: false,
                    total_targets: 1,
                });
            }
            // Multiple targets: capped flat tagged list, no pagination.
            let total_targets = u32::try_from(targets.len()).unwrap_or(u32::MAX);
            let truncated = targets.len() > FIND_USAGES_TARGET_CAP;
            let capped: Vec<ResolvedTarget> =
                targets.into_iter().take(FIND_USAGES_TARGET_CAP).collect();
            let items = collect_multi_target(
                &h,
                &capped,
                &edges,
                limit as usize,
                include_external,
                include_tests,
            )
            .await?;
            Ok(FindUsagesResponse {
                items,
                next: None,
                targets: u32::try_from(capped.len()).unwrap_or(u32::MAX),
                truncated,
                total_targets,
            })
        })
        .await
}

#[cfg(test)]
mod find_usages_unit {
    use super::{passes_narrowing, ref_def_path, FindUsagesArgs};
    use crate::types::SymbolRef;
    use kenn_model::{Kind, Language};

    fn sample_ref() -> SymbolRef {
        SymbolRef {
            id: "cs:Models.Order".into(),
            kind: Kind::Class,
            language: Language::Csharp,
            name: "Order".into(),
            location: Some("./src/Models/Order.cs#10-40".into()),
            package: "Acme.Core".into(),
            module: String::new(),
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
            partial: false,
            via_edge_kind: None,
            direction: None,
        }
    }

    fn args() -> FindUsagesArgs {
        FindUsagesArgs {
            query: "Order".into(),
            kind: None,
            path: None,
            package: None,
            language: None,
            edge_kinds: None,
            include_external: None,
            include_tests: None,
            page_size: None,
            cursor: None,
        }
    }

    #[test]
    fn ref_def_path_strips_prefix_and_line() {
        assert_eq!(
            ref_def_path(&sample_ref()).as_deref(),
            Some("src/Models/Order.cs")
        );
    }

    #[test]
    fn no_filters_pass() {
        assert!(passes_narrowing(&sample_ref(), &args()));
    }

    #[test]
    fn kind_filter_pins_and_rejects() {
        let mut a = args();
        a.kind = Some(vec![Kind::Class]);
        assert!(passes_narrowing(&sample_ref(), &a));
        a.kind = Some(vec![Kind::Method]);
        assert!(!passes_narrowing(&sample_ref(), &a));
    }

    #[test]
    fn language_package_and_path_filters() {
        let mut a = args();
        a.language = Some(vec![Language::Csharp]);
        a.package = Some(vec!["Acme.Core".into()]);
        a.path = Some(vec!["src/Models/Order.cs".into()]);
        assert!(passes_narrowing(&sample_ref(), &a));
        a.path = Some(vec!["./src/Models/Order.cs".into()]);
        assert!(passes_narrowing(&sample_ref(), &a), "leading ./ tolerated");
        a.package = Some(vec!["Other.Pkg".into()]);
        assert!(!passes_narrowing(&sample_ref(), &a));
    }
}
