## Why

The agent-facing symbol search was redesigned to split one vague tool into two —
`find_symbol` (literal identifier) and `search_symbols` (natural-language
intent) — replacing `search_by_intent`. That redesign was **implemented as part
of the Lance search-backend migration**: the two tools, their tiered / blended
ranking, and the `search_by_intent` removal are already in the codebase
(`kenn-mcp/src/tools.rs`, `kenn-store/src/db/reader.rs`).

What never landed:

- The `mcp-symbol-search` **capability spec** — the two-tool contract lives in
  code but not in `openspec/specs/`.
- **Pagination** for `search_symbols` — the tool accepts a `pagination`
  argument and `ListResponse` carries `next`, but the handler hard-codes
  `next: None` ("reserved for the cursor codec extension").
- A **ranking tiebreak** — the design orders `search_symbols` by
  `(score DESC, len(name) ASC, short_id ASC)`; the implementation dropped the
  `len(name)` term and sorts by `(score DESC, short_id ASC)`.
- **Tests** — no MCP-level integration coverage for the two tools, and
  `find_symbol_tiered` / `search_symbols_blended` have no `reader.rs` unit
  tests.

The original change was written against the SurrealDB backend (a `class,camel`
FULLTEXT analyzer, a B-tree on `symbols.name`, a SurrealDB 3.x
multi-FULLTEXT-per-field bug). That backend was deleted; Lance owns search now.
This change is **re-scoped to its genuine remainder**: spec the as-built Lance
surface, finish pagination, fix the tiebreak, and add tests.

## What Changes

- **Land the `mcp-symbol-search` capability** documenting the Lance-backed
  surface as built: `find_symbol`'s four match tiers (`exact` → `prefix` →
  `contains` → `fuzzy`, the last two served by the Lance n-gram name index),
  and `search_symbols`'s blended score
  (`3·name_bm25 + 1·doc_bm25 + 5·substring`).
- **Finish `search_symbols` pagination** using the existing 14-char list
  cursor (`snapshot_id`, `last_short_id`). The result order is a total order
  pinpointed by the unique `short_id`, so no score component and no new cursor
  codec are needed — the `BlendedSearchCursor` from the original design is
  dropped.
- **Restore the `len(name)` tiebreak** in `search_symbols_blended` so ties on
  `score` resolve to the shorter (more specific) name before `short_id`.
- **Add tests**: an MCP integration test over a small indexed fixture, plus
  `reader.rs` unit coverage for the tiering and blend.
- **Drop, as obsolete:** the `class,camel` analyzer and B-tree requirements,
  the SurrealDB multi-FULLTEXT invariant, the `fields` selector on
  `search_symbols` (the blend always considers both name and docs), and the
  `BlendedSearchCursor` two-score codec.

## Capabilities

### New Capabilities

- `mcp-symbol-search`: the agent-facing search surface — the two tools, their
  inputs/outputs, what each result row carries (`match_kind` vs
  `name_score` / `doc_score` / `score`), how the tiers and blend rank, how
  `search_symbols` paginates, and the removal of `search_by_intent`.

The `mcp-server` capability's tool-list requirement (`find_symbol` in place of
`search_by_intent`) is owned by the in-flight `mcp-server` change and is
recorded there — not duplicated as a delta here.

## Impact

- **Code**: `crates/kenn-store/src/db/reader.rs` (pagination + tiebreak in
  `search_symbols_blended`), `crates/kenn-store/src/api/reader.rs` (trait
  signature), `crates/kenn-mcp/src/tools.rs` (`search_symbols` cursor
  emit/parse). New `crates/kenn-mcp/tests/symbol_search.rs`.
- **APIs**: no MCP surface change — `search_by_intent` was already removed.
  `search_symbols` gains working pagination (additive).
- **Schema**: none. The Lance dataset already carries the n-gram name index;
  the redb code graph already carries the `SYMBOLS_BY_NAME` key.
- **Out of scope**: no vector / embedding search, no fuzzy edit-distance, no
  `fields` selector.
