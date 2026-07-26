//! Shared leaf helpers used across the tool modules: view conversions,
//! id/kind/language parsing, location formatting, and the embed/error
//! mapping shims.

use kenn_model::{EdgeKind, Kind, Language};
use kenn_store::api::Reader;
use kenn_store::{BlendedHit, FoundSymbolRow, SymbolRow};

use crate::cursor::DecodedCursor;
use crate::error::{McpError, McpErrorCode};
use crate::types::{
    DefLocation, FindingView, FoundSymbolRef, ImportDirection, RankedFileRef, RankedSymbolRef,
    SearchHitRef, SymbolRef,
};

use super::ReadyView;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Map a store [`Finding`](kenn_store::Finding) into the wire
/// [`FindingView`], carrying the read-time `stale` and `drifted` flags.
pub(crate) fn finding_to_view(f: kenn_store::Finding, stale: bool, drifted: bool) -> FindingView {
    FindingView {
        id: f.id,
        text: f.text,
        tags: f.tags,
        parent_ids: f.parent_ids,
        // `Finding.created_at` is a `Timestamp`; the wire view carries it as its
        // RFC 3339 string.
        created_at: f.created_at.to_string(),
        stale,
        drifted,
    }
}

/// Extract the inclusive 1-based `[start, end]` line span from `content`.
/// Expects 1-based input per `source-data-model` D1 — callers must not
/// pass 0. Out-of-range bounds clamp to the file; an empty result is
/// possible.
pub(crate) fn slice_lines(content: &str, start_line: u32, end_line: u32) -> String {
    debug_assert!(
        start_line >= 1,
        "slice_lines: start_line must be 1-based (got {start_line}); \
         source-data-model D1 stores 1-based lines",
    );
    let start = start_line as usize;
    let end = end_line.max(start_line) as usize;
    content
        .lines()
        .skip(start.saturating_sub(1))
        .take(end.saturating_sub(start) + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn ensure_cursor_matches(h: &ReadyView, c: &DecodedCursor) -> Result<(), McpError> {
    // Only List cursors carry a snapshot directly. TopK cursors are
    // validated via the ResultCache (which is cleared on rotation), so
    // there's nothing to check here for those.
    if let Some(snap) = c.list_snapshot() {
        if snap != h.snapshot_id {
            return Err(McpError::stale_cursor(
                &snap.to_hex(),
                &h.snapshot_id.to_hex(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn split_public_id(id: &str) -> Result<(&'static str, &str), McpError> {
    let prefix = id
        .split_once(':')
        .map(|(p, _)| p)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::new(McpErrorCode::InvalidInput, "id missing language prefix"))?;
    let lang = Language::from_prefix(prefix).ok_or_else(|| {
        McpError::new(
            McpErrorCode::InvalidInput,
            format!("unknown language prefix: `{prefix}`"),
        )
    })?;
    Ok((lang.db_name(), id))
}

pub(crate) async fn hit_to_ref(h: &ReadyView, hit: BlendedHit) -> SearchHitRef {
    match hit {
        BlendedHit::Symbol(r) => {
            let base = symbol_row_to_ref(h, &r.symbol, None, None).await;
            SearchHitRef::Symbol(RankedSymbolRef {
                id: base.id,
                kind: base.kind,
                loc: base.location,
                test: base.test,
                score: r.score,
            })
        }
        BlendedHit::File(f) => SearchHitRef::File(RankedFileRef {
            kind: "file".to_owned(),
            path: f.path,
            score: f.score,
        }),
    }
}

pub(crate) async fn found_to_ref(h: &ReadyView, r: FoundSymbolRow) -> FoundSymbolRef {
    let base = symbol_row_to_ref(h, &r.symbol, None, None).await;
    FoundSymbolRef {
        base,
        match_kind: r.match_kind.as_str().into(),
    }
}

pub(crate) async fn symbol_row_to_ref(
    h: &ReadyView,
    r: &SymbolRow,
    via_edge_kind: Option<EdgeKind>,
    direction: Option<ImportDirection>,
) -> SymbolRef {
    let language = parse_language(&r.language).unwrap_or(Language::Rust);
    let kind = parse_kind(&r.kind).unwrap_or(Kind::Variable);
    let location = first_def_location_string(h, r.id).await;
    let package = if r.pkg_id == 0 {
        String::new()
    } else {
        h.read
            .fetch_package(r.pkg_id)
            .await
            .ok()
            .flatten()
            .map(|p| p.name)
            .unwrap_or_default()
    };
    SymbolRef {
        id: r.pub_id.clone(),
        kind,
        language,
        name: r.name.clone(),
        location,
        package,
        module: String::new(),
        nargs: u8::try_from(r.nargs).unwrap_or(u8::MAX),
        targs: u8::try_from(r.targs).unwrap_or(u8::MAX),
        external: r.external,
        test: r.test,
        partial: r.partial,
        via_edge_kind,
        direction,
    }
}

pub(crate) async fn first_def_location_string(h: &ReadyView, sym_short_id: u32) -> Option<String> {
    let lines = h.read.fetch_def_lines(sym_short_id).await.ok()?;
    // A symbol may carry several def rows — the real definition plus, for some
    // producers, a spurious zero-range occurrence (`file_id != 0` but
    // `start_line == 0`). Stored lines are 1-based (source-data-model D1), so
    // pick the first row with a genuine location and ignore the rest. `None`
    // when there is no anchored def at all (e.g. a synthetic module symbol).
    let def = lines
        .into_iter()
        .find(|d| d.file_id != 0 && d.start_line >= 1)?;
    let path = h.read.fetch_file_path(def.file_id).await.ok().flatten()?;
    if def.start_line == def.end_line {
        Some(format!("./{path}#{}", def.start_line))
    } else {
        Some(format!("./{path}#{}-{}", def.start_line, def.end_line))
    }
}

pub(crate) async fn defs_for_symbol(h: &ReadyView, sym_short_id: u32) -> Vec<DefLocation> {
    let Ok(lines) = h.read.fetch_def_lines(sym_short_id).await else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(lines.len());
    for d in lines {
        // Skip non-anchored (file_id == 0) and spurious zero-range
        // (start_line == 0) defs — see `first_def_location_string`.
        if d.file_id == 0 || d.start_line < 1 {
            continue;
        }
        let Ok(Some(path)) = h.read.fetch_file_path(d.file_id).await else {
            continue;
        };
        out.push(DefLocation {
            file: path,
            start_line: d.start_line,
            end_line: d.end_line,
        });
    }
    out
}

pub(crate) fn parse_language(s: &str) -> Option<Language> {
    // Delegate to the model's canonical mapping (the inverse of `db_name`) so a
    // new language (e.g. markdown) is never silently missed here.
    Language::from_db_name(s)
}

pub(crate) fn parse_kind(s: &str) -> Option<Kind> {
    // Delegate to the model's canonical mapping (the inverse of `db_name`)
    // rather than re-listing every variant here — a hand-rolled copy silently
    // omitted the markdown `document`/`section` kinds.
    Kind::from_db_name(s)
}

pub(crate) fn internal(e: impl std::fmt::Display) -> McpError {
    McpError::new(McpErrorCode::InternalError, e.to_string())
}

/// Convert a storage error to MCP, mapping `EmbedderStarting` to the
/// `EMBEDDER_STARTING` retry signal (matching the agent contract used
/// for `INDEX_UNAVAILABLE`). All other variants surface as
/// `INTERNAL_ERROR`. Use this at search-tool boundaries that may invoke
/// the embedder; for non-embed call paths the plain [`internal`] helper
/// is fine.
pub(crate) fn db_to_mcp(e: kenn_store::api::DbError) -> McpError {
    match e {
        kenn_store::api::DbError::EmbedderStarting(reason) => McpError::embedder_starting(&reason),
        other => internal(other),
    }
}

/// Embed a query string through the process-global embedder, mapping
/// failure modes to MCP errors. `Ok(None)` means embedding is configured
/// but the query returned no vector (model unavailable) — callers
/// degrade to lexical-only by passing `None` to the search call.
pub(crate) async fn embed_query(query: &str) -> Result<Option<Vec<f32>>, McpError> {
    use kenn_store::embed::EmbedError;
    match kenn_store::shared_embedder().embed_query(query).await {
        Ok(v) => Ok(v),
        Err(EmbedError::Starting(reason)) => Err(McpError::embedder_starting(&reason)),
        Err(other) => Err(internal(other)),
    }
}

/// Take one page of an already-ordered, in-memory axis listing.
///
/// The atlas axes are computed whole (they are small and their ordering is
/// deterministic for a snapshot), so pagination is a plain offset walk rather
/// than a cache-backed one: the cursor carries the offset, and the snapshot id
/// in it makes a cursor from a previous index run fail loudly instead of
/// silently paging a different corpus.
///
/// Render caps are NOT applied here. A cap is presentation policy for a page
/// with a reader; a query must be able to reach every entity, which is what
/// paging is for (design D8).
pub(crate) fn page_axis_items<T>(
    mut items: Vec<T>,
    pagination: Option<&crate::types::Pagination>,
    snapshot: crate::cursor::SnapshotId,
) -> Result<(Vec<T>, Option<String>), crate::error::McpError> {
    let page_size = pagination
        .and_then(|p| p.page_size)
        .unwrap_or(crate::types::DEFAULT_PAGE)
        .clamp(1, crate::types::MAX_PAGE) as usize;

    let offset = match pagination.and_then(|p| p.cursor.as_deref()) {
        None => 0usize,
        Some(c) => {
            let decoded = crate::cursor::decode_cursor(c)?;
            if decoded.list_snapshot() != Some(snapshot) {
                return Err(crate::error::McpError::new(
                    crate::error::McpErrorCode::InvalidInput,
                    "cursor is from a different snapshot; restart the listing",
                ));
            }
            match decoded {
                crate::cursor::DecodedCursor::List { last_short_id, .. } => last_short_id as usize,
                _ => 0,
            }
        }
    };

    if offset >= items.len() {
        return Ok((Vec::new(), None));
    }
    let rest = items.split_off(offset);
    let has_more = rest.len() > page_size;
    let page: Vec<T> = rest.into_iter().take(page_size).collect();
    let next = has_more.then(|| {
        let next_offset = u32::try_from(offset + page_size).unwrap_or(u32::MAX);
        crate::cursor::encode_list_cursor(snapshot, next_offset)
    });
    Ok((page, next))
}

#[cfg(test)]
mod page_axis_tests {
    use super::page_axis_items;
    use crate::cursor::{decode_cursor, DecodedCursor, SnapshotId};
    use crate::types::Pagination;

    fn snap() -> SnapshotId {
        SnapshotId([1, 2, 3, 4, 5, 6])
    }

    fn page(size: u32, cursor: Option<String>) -> Pagination {
        Pagination {
            page_size: Some(size),
            cursor,
        }
    }

    /// Draining an axis returns every entity exactly once, in order — no gap at
    /// a page boundary, no repeat. Mutation-checked: an off-by-one in the offset
    /// (`offset + page_size - 1`) duplicates a row and this fails.
    #[test]
    fn draining_returns_every_item_exactly_once() {
        let all: Vec<u32> = (0..10).collect();
        let mut seen: Vec<u32> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (items, next) =
                page_axis_items(all.clone(), Some(&page(3, cursor.clone())), snap()).unwrap();
            seen.extend(items);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, all, "every item once, in order");
    }

    /// A listing that fits in one page carries no cursor — a `next` that leads
    /// nowhere invites a pointless round-trip.
    #[test]
    fn a_single_page_has_no_cursor() {
        let (items, next) = page_axis_items(vec![1, 2, 3], Some(&page(25, None)), snap()).unwrap();
        assert_eq!(items.len(), 3);
        assert!(next.is_none());
    }

    /// A cursor minted against another snapshot must fail loudly rather than
    /// silently page a different corpus.
    #[test]
    fn a_cursor_from_another_snapshot_is_rejected() {
        let (_, next) = page_axis_items(vec![1, 2, 3, 4], Some(&page(2, None)), snap()).unwrap();
        let c = next.expect("a second page exists");
        // Same cursor, different snapshot.
        let other = SnapshotId([9, 9, 9, 9, 9, 9]);
        let err = page_axis_items(vec![1, 2, 3, 4], Some(&page(2, Some(c.clone()))), other)
            .expect_err("stale cursor must error");
        assert!(format!("{err}").contains("different snapshot"), "{err}");
        // And it still works against the snapshot it was minted for.
        let (items, _) =
            page_axis_items(vec![1, 2, 3, 4], Some(&page(2, Some(c.clone()))), snap()).unwrap();
        assert_eq!(items, vec![3, 4], "the second page");
        assert!(matches!(
            decode_cursor(&c).unwrap(),
            DecodedCursor::List { .. }
        ));
    }
}
