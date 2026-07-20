## 1. Audit callers

- [x] 1.1 Grep for callers of `search_symbols` (the MCP tool) and
  `tools::search_symbols`. Result: no internal Rust callers beyond
  `server.rs:86` dispatcher and tests.
- [x] 1.2 Grep for callers of `search_findings`. Result: tool layer
  has only `semantic_search` (`tools.rs:1381`) and the dispatcher
  as internal callers.
- [x] 1.3 Grep for callers of `SearchSymbolsArgs::pagination` and
  `count_only`. Result: `count_only` lives on `search_symbols` AND
  on `list_callers/callees/in_scope/usages/imports` args. No internal
  callers, only tests — drop confirmed.

## 2. Constants and helpers (D8)

- [x] 2.1 In `crates/kenn-mcp/src/types.rs`: replace
  `DEFAULT_PAGE_LIMIT`/`MAX_PAGE_LIMIT` with:
  ```rust
  pub const DEFAULT_PAGE: u32 = 25;
  pub const MAX_PAGE: u32 = 50;
  pub const DEFAULT_TOP_K_PAGE: u32 = 10;
  pub const MAX_TOP_K_PAGE: u32 = 30;
  pub const TOP_K_MATERIALIZE: u32 = 30;
  ```
- [x] 2.2 Replace `clamp_limit` with `clamp_page` and
  `clamp_top_k_page`. No `derive_page_size` — the agent picks, the
  server only clamps.
- [x] 2.3 Add a `const _: () = assert!(TOP_K_MATERIALIZE <= 256);`
  static assertion (the pool ceiling from D7) so future bumps fail
  loudly.
- [x] 2.4 Update the `Pagination` struct docstring at
  `types.rs:154-163` to reflect the new semantics (page_size, not
  desired-total).
- [x] 2.5 Replace the existing `clamp_limit_enforces_bounds` test with:
  - `clamp_page_enforces_bounds`: None → 25, 0 → 1, 50 → 50, 9999 → 50.
  - `clamp_top_k_page_enforces_bounds`: None → 10, 0 → 1, 20 → 20,
    9999 → 30.

## 3. Pagination args: drop `limit`, add `page_size`

- [x] 3.1 In `types.rs`, change `Pagination` struct from
  `{ limit: Option<u32>, cursor: Option<String> }` to
  `{ page_size: Option<u32>, cursor: Option<String> }`.
- [x] 3.2 In `tools.rs`, drop the `count_only: Option<bool>` field
  from `SearchSymbolsArgs`, `ListCallersArgs`, `ListCalleesArgs`,
  `ListUsagesArgs`, `ListInScopeArgs`, `ListImportsArgs`, and any
  other args struct that has it. Drop the related branches inside
  each tool body.
- [x] 3.3 For tools that take `args.limit: Option<u32>` directly
  (not via `Pagination`) — `FindSymbolArgs`, `FindAtLocationArgs`,
  `FindSimilarArgs`, `SearchFindingsArgs`, `SemanticSearchArgs` —
  rename to `page_size: Option<u32>` and adjust call sites.
- [x] 3.4 Update every paginated tool's body to use `clamp_page` or
  `clamp_top_k_page` (per family). For iteration tools the call site
  becomes `clamp_page(args.pagination.as_ref().and_then(|p| p.page_size))`;
  for top-K it's `clamp_top_k_page(...)`.

## 4. Response shape — drop `total`

- [x] 4.1 Drop the `total: u64` field from `ListResponse<T>` in
  `types.rs`. Shape becomes `{ items, next }`.
- [x] 4.2 Drop the `total` field assembly from every paginated tool
  in `tools.rs`.
- [x] 4.3 Update the `list_response_roundtrips_json` test in
  `types.rs` for the new shape.

## 5. Cursor — rename `Search` to `TopK`, reshape

- [x] 5.1 In `cursor.rs`, rename `DecodedCursor::Search` → `TopK`
  and reshape its fields from `{ snapshot, last_score, last_short_id }`
  to `{ cache_id: [u8; 16], offset: u32 }`. Old `Search` variant was
  never emitted (today's `search_symbols` emits `List`), so the
  rename is internal-only.
- [x] 5.2 Replace `SEARCH_CURSOR_BYTES = 14` with `TOPK_CURSOR_BYTES = 20`.
  Replace `encode_search_cursor` → `encode_topk_cursor`. Update the
  `decode_cursor` length arm.
- [x] 5.3 `DecodedCursor::List` unchanged.
- [x] 5.4 Update the `search_cursor_round_trips` test to round-trip
  a `TopK` cursor: `cache_id [0xAB; 16]`, `offset 8` → encode → decode →
  same. Add an assertion that the old 14-byte ex-Search blob fails to
  decode with the new length arm.

## 6. Reader API contract (D10)

- [x] 6.1 Change `search_symbols_blended` signature: drop the `u64`
  total return AND drop `cursor_after`. Signature becomes
  `(query, target_total, include_external, include_tests) -> Vec<BlendedSymbolRow>`.
- [x] 6.2 Remove the cursor-drain block (today lines 489-496) and
  the `total` accumulator (lines 438-444). Ordering unchanged.
- [x] 6.3 Replace `pool = max(8·limit, 64)` with
  `pool = min(2·limit, 256).max(64)` per D7. Document the formula
  in the docstring.
- [x] 6.4 Update the `api/reader.rs` trait signature to match.
- [x] 6.5 Add a unit test asserting `pool ≤ 256` for all
  `target_total ∈ [1, 256]` and `pool ≥ 64` for all
  `target_total ∈ [1, 32]`.
- [x] 6.6 Update `crates/kenn-store/tests/storage_fixtures.rs` tests
  that pattern-match on the 2-tuple return.
- [x] 6.7 Update `crates/kenn-store/benches/storage_harness.rs`
  `bench_search_symbols_blended` to match the new signature.

## 7. Result cache (D12)

- [x] 7.1 Add `crates/kenn-mcp/src/result_cache.rs`. Define
  `ResultCache<T>` parametric over the cached row type, backed by
  `Mutex<LruCache<CacheId, CachedTopK<T>>>` where
  `CachedTopK<T> { snapshot: SnapshotId, rows: Vec<T> }`. Bound
  N = 64. No TTL. API:
  - `fn put(snapshot, rows) -> CacheId`
  - `fn slice(id, offset, page_size) -> Result<(Vec<T>, usize), McpError>`
    (returns cloned rows + total length; STALE_CURSOR on miss or
    snapshot mismatch)
  - `fn put_and_take_first_page(snapshot, rows, page_size) -> (CacheId, Vec<T>)`
  - `fn clear()` (called on snapshot rotation)
- [x] 7.2 Use a crate dep for LRU (`lru` is the standard pick;
  check `mcp__dependency__get_latest_version`).
  `CacheId` is a random 16-byte array generated via `rand`.
- [x] 7.3 Wire two instantiations into `ServerState`:
  `ResultCache<RankedSymbolRef>` and `ResultCache<RankedFindingView>`.
  Plumb refs through `state.with_db` / `state.with_findings`.
- [x] 7.4 Snapshot rotation: find the existing rotate hook in
  `ServerState` / `state.rs` and call `clear()` on both caches.
- [x] 7.5 Unit tests in `result_cache.rs`:
  - `put_then_slice_round_trips`.
  - `lru_eviction_at_bound`: insert 65, oldest is gone (`slice`
    returns `STALE_CURSOR`).
  - `clear_drops_all`.
  - `slice_unknown_id_returns_stale_cursor`.
  - `slice_wrong_snapshot_returns_stale_cursor`.

## 8. `search_symbols` refactor (D12)

- [x] 8.1 In `search_symbols` (`tools.rs:814`):
  - **First call** (no cursor): call `search_symbols_blended` once
    with `TOP_K_MATERIALIZE = 30`. If `rows.len() <= page_size`,
    return single-shot (no cursor, no cache entry). Else call
    `cache.put_and_take_first_page(snapshot, rows, page_size)` and
    emit a `TopK` cursor.
  - **Continuation** (cursor present, decoded as `TopK`):
    `cache.slice(cache_id, offset, page_size)` → `STALE_CURSOR` if
    missing; emit a `TopK` cursor only if `offset + page.len() < total_len`.
- [x] 8.2 Update tool description string at `server.rs` to match
  D9: "`page_size` is rows per response (default 10, max 30 —
  server materializes top 30 ranked results; cursor walks within
  them)."

## 9. `search_findings` refactor (D11 + D12)

- [x] 9.1 Mirror the same refactor in `search_findings`
  (`tools.rs:1483`). Pass `TOP_K_MATERIALIZE` to
  `store.search_findings()`. The returned Vec becomes the cache
  entry's rows.
- [x] 9.2 Update tool description string to match search_symbols
  wording (same envelope).
- [x] 9.3 Verify `store.search_findings(query, limit, resolver)` has
  no internal pool/over-fetch problem at `limit = 30` analogous to
  D7's bug. If unbounded, file a follow-up (do NOT silently tighten
  in this change).

## 10. `semantic_search` adjustment

- [x] 10.1 `semantic_search` is single-shot (no pagination). Update
  its `limit` (now `page_size`) clamp to use `clamp_top_k_page` for
  symmetry with the other top-K tools. No cursor logic.
- [x] 10.2 Description string update: "`page_size` is rows per
  response (default 10, max 30); single-shot, no pagination."

## 11. Iteration tool refactors

- [x] 11.1 Switch every iteration tool's call site
  (`list_callers/callees/usages/in_scope/implementers/overrides/
  correspondences/imports/module_files`, `find_similar`, `find_symbol`)
  to `clamp_page(args.pagination.as_ref().and_then(|p| p.page_size))`.
- [x] 11.2 Cursor emission unchanged (`List` variant, snap + short_id).
  Cursor terminates only when the corpus exhausts (today's behaviour
  for iteration tools).
- [x] 11.3 Update each iteration tool's description string to:
  "`page_size` is rows per response (default 25, max 50); cursor
  walks the full corpus until exhaustion."
- [x] 11.4 Fix stale "default 20" strings at `tools.rs:732` and
  `:876`. Update `cursor.rs:9` module doc and `types.rs:156`
  Pagination struct doc.

## 12. Tests

- [x] 12.1 Update existing pagination tests in
  `crates/kenn-mcp/tests/symbol_search.rs`:
  `search_symbols_paginates_without_gaps`,
  `search_symbols_rejects_a_stale_cursor`,
  `search_symbols_rejects_a_malformed_cursor`,
  `search_symbols_final_page_emits_no_cursor`. Update assertions for
  new shape (`page_size` arg, top-K cap=30 materialize, cache-slice
  exhaustion).
- [x] 12.2 Add `search_symbols_caches_first_call`: confirm
  reader is called once across page 1 + page 2 (spy via test
  fixture). Cache amortization is the load-bearing perf claim.
- [x] 12.3 Add `search_symbols_cursor_stale_after_clear`: clear
  cache mid-walk, continuation returns `-32602 STALE_CURSOR`.
- [x] 12.4 Delete `search_count_only_returns_zero_with_empty_items`
  in `tests/end_to_end.rs` (count_only is removed).
- [x] 12.5 Delete `list_usages_count_only_returns_total_without_items`
  in `tests/navigation.rs` (count_only is removed).
- [x] 12.6 Add `search_findings_paginates_to_cap`: parallel of 12.1
  for findings.
- [x] 12.7 Iteration tools: update tests that depended on `limit` as
  page-size; now `page_size`. Add a test for an iteration tool
  walking past 50 rows (cursor keeps walking, no server cap).

## 13. Documentation

- [x] 13.1 Update `mcp-symbol-search` capability spec per the
  `specs/mcp-symbol-search/spec.md` delta in this change.
- [x] 13.2 Update `claude-plugins/kenn/skills/kenn/SKILL.md`
  ("Pagination" section): the agent's only knob is `page_size`,
  family defaults are 10 (top-K) / 25 (iteration), iteration walks
  the corpus without a server-side total cap.
- [x] 13.3 Cross-reference `mcp-pagination-spec-alignment` in the
  modified spec.

## 14. Verification

- [x] 14.1 `cargo clippy --workspace --all-targets` clean.
- [x] 14.2 `cargo test --workspace` clean.
- [x] 14.3 `just crap-ci` passes.
- [x] 14.4 Manual smoke against Claude Code: rebuild, reload MCP,
  call `search_symbols` with `page_size=5`. Confirm response has no
  `total`, has `next` cursor, returns 5 items. Walk cursor twice
  more (5+5+ remainder = up to 30 across 3 calls).
- [x] 14.5 `openspec validate search-symbols-top-k` clean.
