use kenn_model::{EdgeKind, FieldOp, Language};
use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor, encode_list_cursor, DecodedCursor};
use crate::error::{QueryError, QueryErrorCode};
use crate::types::{
    clamp_page, FileRef, Filters, ImportDirection, ListResponse, Pagination, SymbolRef,
};

use crate::ctx::QueryCtx;
use crate::{ensure_cursor_matches, internal, parse_language, split_public_id, symbol_row_to_ref};

// ── NAVIGATE / SCOPE  (uniform args + dispatch) ─────────────────────────────

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ByIdArgs {
    pub id: String,
    #[serde(default)]
    pub filters: Option<Filters>,
    #[serde(default)]
    pub pagination: Option<Pagination>,
}

async fn list_relation(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
    relation: &'static str,
    direction: RelationDirection,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    let limit = clamp_page(args.pagination.as_ref().and_then(|p| p.page_size));
    let filters = args.filters.clone().unwrap_or_default();
    // Universal default: exclude test and external symbols unless the caller
    // opts in (`Filters.include_tests` / `include_external`). Matches the CLI's
    // universal default and the search tools; overridable per call.
    // Every predicate the caller set, not just visibility: `package`, `kind`
    // and `language` used to be accepted here and silently dropped, so a
    // narrowed `kenn list` returned the unnarrowed list. Applied store-side,
    // before `limit`, so a filtered page is still a full page.
    let narrow = kenn_store::RowNarrow {
        include_external: filters.include_external.unwrap_or(false),
        include_tests: filters.include_tests.unwrap_or(false),
        packages: filters.package.clone(),
        kinds: filters
            .kind
            .as_ref()
            .map(|ks| ks.iter().map(|k| k.db_name().to_string()).collect()),
        languages: filters
            .language
            .as_ref()
            .map(|ls| ls.iter().map(|l| l.db_name().to_string()).collect()),
    };
    let cursor = if let Some(c) = args.pagination.as_ref().and_then(|p| p.cursor.as_ref()) {
        Some(decode_cursor(c)?)
    } else {
        None
    };
    let (lang, native) = split_public_id(&args.id)?;
    let native = native.to_string();
    if let Some(c) = cursor.as_ref() {
        ensure_cursor_matches(ctx.snapshot_id, c)?;
    }
    let row = ctx
        .read
        .fetch_symbol(lang, &native)
        .await
        .map_err(internal)?;
    let Some(target) = row else {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!(
                "no symbol with id `{native}` in the current index — \
                         use find_symbol or search_symbols to locate it"
            ),
        ));
    };
    let cursor_after = match cursor {
        Some(DecodedCursor::List { last_short_id, .. }) => Some(last_short_id),
        _ => None,
    };
    let (rows, total) = match direction {
        RelationDirection::Inbound => {
            ctx.read
                .list_inbound(target.id, relation, limit, cursor_after, &narrow)
                .await
        }
        RelationDirection::Outbound => {
            ctx.read
                .list_outbound(target.id, relation, limit, cursor_after, &narrow)
                .await
        }
    }
    .map_err(internal)?;
    let returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let next = if returned == limit && u64::from(returned) < total {
        rows.last()
            .map(|r| encode_list_cursor(ctx.snapshot_id, r.id))
    } else {
        None
    };
    let mut items: Vec<SymbolRef> = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(symbol_row_to_ref(ctx.read, &r, None, None).await);
    }
    Ok(ListResponse { items, next })
}

#[derive(Copy, Clone)]
enum RelationDirection {
    Inbound,
    Outbound,
}

pub async fn list_callers(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    list_relation(ctx, args, "calls", RelationDirection::Inbound).await
}
pub async fn list_callees(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    list_relation(ctx, args, "calls", RelationDirection::Outbound).await
}
pub async fn list_implementers(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    list_relation(ctx, args, "implements", RelationDirection::Inbound).await
}
pub async fn list_overrides(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    list_relation(ctx, args, "overrides", RelationDirection::Inbound).await
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListUsagesArgs {
    pub id: String,
    #[serde(default)]
    pub edge_kinds: Option<Vec<EdgeKind>>,
    #[serde(default)]
    pub op_filter: Option<FieldOp>,
    #[serde(default)]
    pub filters: Option<Filters>,
    #[serde(default)]
    pub pagination: Option<Pagination>,
}

pub async fn list_usages(
    ctx: &QueryCtx<'_>,
    args: &ListUsagesArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    let kinds = args.edge_kinds.clone().unwrap_or_else(|| {
        vec![
            EdgeKind::Calls,
            EdgeKind::TypeUse,
            EdgeKind::FieldAccess,
            EdgeKind::Instantiates,
            // Table references. Inbound-only by nature — nothing points AT a
            // function with one — so for a code symbol these contribute
            // nothing, and for a table node they are the entire answer. Left
            // out, `list usages` on a table returns empty while the graph holds
            // the edges, which reads as "no code touches this".
            EdgeKind::DefinesTable,
            EdgeKind::AltersTable,
            EdgeKind::AccessesTable,
        ]
    });
    let mut all_items: Vec<SymbolRef> = Vec::new();
    for k in kinds {
        let by_id = ByIdArgs {
            id: args.id.clone(),
            filters: args.filters.clone(),
            pagination: None,
        };
        let resp = list_relation(ctx, &by_id, k.db_name(), RelationDirection::Inbound).await?;
        for mut item in resp.items {
            if k != EdgeKind::FieldAccess && args.op_filter.is_some() {
                // ignore op_filter for non-field_access kinds
            }
            item.via_edge_kind = Some(k);
            all_items.push(item);
        }
    }
    let limit = clamp_page(args.pagination.as_ref().and_then(|p| p.page_size)) as usize;
    if all_items.len() > limit {
        all_items.truncate(limit);
    }
    Ok(ListResponse {
        items: all_items,
        next: None,
    })
}

pub async fn list_correspondences(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    let inbound = list_relation(ctx, args, "corresponds_to", RelationDirection::Inbound).await?;
    let outbound = list_relation(ctx, args, "corresponds_to", RelationDirection::Outbound).await?;
    let mut items = inbound.items;
    items.extend(outbound.items);
    Ok(ListResponse { items, next: None })
}

pub async fn list_in_scope(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    list_relation(ctx, args, "defined_in", RelationDirection::Inbound).await
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListImportsArgs {
    pub id: String,
    pub direction: ImportDirectionArg,
    #[serde(default)]
    pub kind: Option<Vec<String>>,
    #[serde(default)]
    pub filters: Option<Filters>,
    #[serde(default)]
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportDirectionArg {
    Outbound,
    Inbound,
    Both,
}

pub async fn list_imports(
    ctx: &QueryCtx<'_>,
    args: &ListImportsArgs,
) -> Result<ListResponse<SymbolRef>, QueryError> {
    let by_id = ByIdArgs {
        id: args.id.clone(),
        filters: args.filters.clone(),
        pagination: args.pagination.clone(),
    };
    match args.direction {
        ImportDirectionArg::Outbound => {
            list_relation(ctx, &by_id, "imports", RelationDirection::Outbound).await
        }
        ImportDirectionArg::Inbound => {
            list_relation(ctx, &by_id, "imports", RelationDirection::Inbound).await
        }
        ImportDirectionArg::Both => {
            let mut o = list_relation(ctx, &by_id, "imports", RelationDirection::Outbound).await?;
            let mut i = list_relation(ctx, &by_id, "imports", RelationDirection::Inbound).await?;
            for item in &mut o.items {
                item.direction = Some(ImportDirection::Outbound);
            }
            for item in &mut i.items {
                item.direction = Some(ImportDirection::Inbound);
            }
            o.items.extend(i.items);
            Ok(ListResponse {
                items: o.items,
                next: None,
            })
        }
    }
}

pub async fn list_module_files(
    ctx: &QueryCtx<'_>,
    args: &ByIdArgs,
) -> Result<ListResponse<FileRef>, QueryError> {
    let limit = clamp_page(args.pagination.as_ref().and_then(|p| p.page_size));
    let cursor = if let Some(c) = args.pagination.as_ref().and_then(|p| p.cursor.as_ref()) {
        Some(decode_cursor(c)?)
    } else {
        None
    };
    let (lang, native) = split_public_id(&args.id)?;
    let native = native.to_string();
    if let Some(c) = cursor.as_ref() {
        ensure_cursor_matches(ctx.snapshot_id, c)?;
    }
    let Some(module) = ctx
        .read
        .fetch_symbol(lang, &native)
        .await
        .map_err(internal)?
    else {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!("no module/package with id `{native}` in the current index"),
        ));
    };
    let cursor_after = match cursor {
        Some(DecodedCursor::List { last_short_id, .. }) => Some(last_short_id),
        _ => None,
    };
    let (rows, total) = ctx
        .read
        .list_module_files(module.id, limit, cursor_after)
        .await
        .map_err(internal)?;
    let returned = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let next = if returned == limit && u64::from(returned) < total {
        rows.last()
            .map(|r| encode_list_cursor(ctx.snapshot_id, r.id))
    } else {
        None
    };
    let items: Vec<FileRef> = rows
        .into_iter()
        .map(|r| FileRef {
            path: r.path,
            language: parse_language(&r.language).unwrap_or(Language::Rust),
            test: r.test,
            external: r.external,
        })
        .collect();
    Ok(ListResponse { items, next })
}
