## 1. Already delivered (Lance search-backend migration)

Listed for the archive record — these shipped with the Lance migration.

- [x] 1.1 `find_symbol` MCP tool + `find_symbol_tiered` store method: four
      tiers (`exact` / `prefix` via redb `SYMBOLS_BY_NAME` key, `contains` /
      `fuzzy` via the Lance n-gram name index).
- [x] 1.2 `search_symbols` MCP tool + `search_symbols_blended` store method:
      `3·name_bm25 + 1·doc_bm25 + 5·substring` over the Lance n-gram and
      stemmed indexes.
- [x] 1.3 `search_by_intent` removed from `tools.rs` and `server.rs`.
- [x] 1.4 Result rows carry `match_kind` (`find_symbol`) and
      `name_score` / `doc_score` / `score` (`search_symbols`).

## 2. search_symbols pagination

- [x] 2.1 Add a `cursor_after: Option<ShortId>` parameter to
      `search_symbols_blended` in `api/reader.rs` (trait) and `db/reader.rs`
      (impl), mirroring `list_inbound`. Re-blend the Lance top-k pool, sort by
      `(score DESC, len(name) ASC, short_id ASC)`, and drop every row up to and
      including the boundary `short_id`. The pool stays at `limit · 8`, fixing
      the maximum reachable page depth.
- [x] 2.2 Wire `tools::search_symbols` to decode an incoming
      `pagination.cursor` (existing 14-char list cursor) into `cursor_after`,
      and to emit `next` via `encode_list_cursor` from the last returned row.
- [x] 2.3 Restart pagination cleanly when the cursor's `snapshot_id` no longer
      matches the active snapshot (existing `STALE_CURSOR` error path via
      `ensure_cursor_matches`).

## 3. Ranking tiebreak

- [x] 3.1 Add the `len(name) ASC` term to the `search_symbols_blended` sort so
      ties resolve `(score DESC, len(name) ASC, short_id ASC)`.

## 4. Tests

- [x] 4.1 `crates/kenn-mcp/tests/symbol_search.rs`: builds an in-process
      corpus + a `Ready` `ServerState`, asserts `find_symbol` exact-then-tier
      ordering and `search_symbols` score-ranked rows.
- [x] 4.2 `search_symbols` pagination tests: store-level
      (`blended_pagination_reproduces_the_single_page_order`) and MCP-level
      (`search_symbols_paginates_without_gaps`) confirm paged traversal equals
      the single-page set; `search_symbols_rejects_a_stale_cursor` covers the
      stale-snapshot path.
- [x] 4.3 Tier classification covered by
      `find_symbol_classifies_and_orders_tiers`; blend formula and
      `include_tests` / `include_external` filters were already covered by the
      pre-existing `search_correctness.rs` suite.

## 5. Documentation

- [x] 5.1 Fixed the stale `MatchKind` doc comments in
      `crates/kenn-store/src/api/types.rs` — rewritten for the Lance n-gram /
      redb-key tiers.
- [x] 5.2 Updated `crates/kenn-mcp/README.md` (`search_by_intent` →
      `find_symbol`) and the `camel+class` references in the `find_symbol`
      tool description (`server.rs`) and doc comment (`tools.rs`).
      Note: `docs/kenn/store-architecture.md` is broadly stale from the
      single-backend collapse (describes the `surreal` default + dual-backend
      feature flags) — out of scope for this change.
