## Context

`find_symbol` and `search_symbols` are the agent-facing symbol-search MCP
tools. They were redesigned to replace a single vague tool (plus
`search_by_intent`), and the redesign shipped with the Lance search-backend
migration. The surface is in code; what remains is to spec it and finish three
loose ends — pagination, a ranking tiebreak, and tests.

The Lance dataset carries three inverted indexes over the symbol table:

- `doc_text` — stemmed BM25 word index (prose / docstring search).
- `name_text` — n-gram index (3–4 grams) over symbol names; covers substring
  and fuzzy matching, subsuming the old O(n) substring scan.
- `name` — raw-keyword index, the whole-name exact-match partner.

`find_symbol_tiered` and `search_symbols_blended` (in `kenn-store/src/db/
reader.rs`) sit on top of these; `kenn-mcp/src/tools.rs` wraps them. Exact and
prefix name lookup are served by the redb `SYMBOLS_BY_NAME` key, not Lance.

## Goals / Non-Goals

**Goals:**

- The `mcp-symbol-search` capability spec matches the code as built.
- `search_symbols` paginates with a stable cursor.
- `search_symbols` tie ordering is deterministic and documented.
- Both store methods (`find_symbol_tiered`, `search_symbols_blended`) gain
  regression coverage.

**Non-Goals:**

- No vector / embedding search — the `embedding` index is wired but dormant
  until the embedding-producer follow-up.
- No fuzzy edit-distance / typo correction beyond what the n-gram index gives.
- No `fields` selector — the blend always weighs both name and docstrings; a
  name-only or docs-only mode would be a separate tool, not a parameter.
- No `find_symbol` pagination — a literal-name lookup is a small result set by
  design; the agent restates the query rather than paging.

## Decisions

### The two-tool surface (as built)

```
        ┌──────────────────────────┬───────────────────────────┐
        │  find_symbol(name, …)    │  search_symbols(query, …)  │
        ├──────────────────────────┼───────────────────────────┤
        │ "I have a name."         │ "I have an intent."        │
        │                          │                            │
        │ Tiers, in order:         │ Two BM25 lists:            │
        │  1. exact   redb key     │  name_bm25 ← search_names  │
        │  2. prefix  redb range   │  doc_bm25  ← search_docs   │
        │  3. contains Lance ngram │                            │
        │  4. fuzzy   Lance ngram  │ score = 3·name_bm25        │
        │                          │       + 1·doc_bm25         │
        │ Order: match_kind ASC,   │       + 5·(substring?1:0)  │
        │  len(name) ASC,          │                            │
        │  short_id ASC.           │ Order: score DESC,         │
        │                          │  len(name) ASC,            │
        │ Each row carries         │  short_id ASC.             │
        │ `match_kind`.            │                            │
        │                          │ Each row carries           │
        │                          │ name_score, doc_score,     │
        │                          │ score.                     │
        └──────────────────────────┴───────────────────────────┘
```

`find_symbol`'s `contains` tier is a Lance n-gram hit whose name contains the
query as a substring; `fuzzy` is an n-gram hit with no substring containment
(e.g. `Foo.Bar.OrderHandler.M` for query `order handler`).

### Pagination cursor: reuse the existing list cursor

The result order `(score DESC, len(name) ASC, short_id ASC)` is a total order.
`short_id` is globally unique, so it alone pinpoints the boundary row — no
score component is needed in the cursor. The existing list-cursor codec already
carries `(snapshot_id[6], last_short_id[4])` = 14 base64 chars. So:

- **No new codec, and the search cursor is the wrong fit.** A `last_score`
  field cannot resolve ties between rows of equal score (common — every
  pure-substring match scores exactly `5.0`), since the cursor cannot also
  carry `len(name)`. Locating the boundary by the unique `short_id` is both
  correct and consistent with every other paginated tool (`list_callers`,
  `list_usages`, `list_module_files`), all of which use the list cursor.
- `search_symbols_blended` gains a `cursor_after: Option<ShortId>` parameter,
  mirroring `list_inbound` / `list_outbound`. It re-blends the Lance top-k
  pool, sorts by the total order, drops every row up to and including the
  boundary `short_id`, and returns the next `limit` rows.

The Lance pool stays at `limit · 8` (the existing `search_symbols_by_name`
constant). That fixes the maximum reachable page depth at 8 — the boundary row
must still be within the re-blended pool. Agents rarely page far into a
relevance list, so this is acceptable; a deeper page returns empty and the
agent stops.

### Tiebreak: len(name) before short_id

`search_symbols_blended` currently sorts `(score DESC, short_id ASC)`. The
shorter name is almost always the more specific answer — `OrderHandler` (12
chars) should beat `Foo.Bar.OrderHandler.Method` (27) at equal score. Restore
the documented `(score DESC, len(name) ASC, short_id ASC)`. The `short_id`
term keeps the order total, so the cursor continuation stays exact.

### What is dropped from the original design

| Original (SurrealDB) decision            | Re-scoped resolution                         |
|------------------------------------------|----------------------------------------------|
| `class,camel` FULLTEXT analyzer          | Lance n-gram name index — already built      |
| B-tree on `symbols.name`                 | redb `SYMBOLS_BY_NAME` key — already built   |
| SurrealDB 3.x multi-FULLTEXT-per-field bug| No SurrealDB; not applicable                 |
| `fields` selector on `search_symbols`    | Blend always weighs name + docs              |
| `BlendedSearchCursor` (name+doc scores)  | Single composite score → existing 20-char cursor |

## Risks / Trade-offs

- **Deep pagination cost.** Each page re-blends a larger Lance pool. Bounded by
  a max depth; agents rarely page far. Acceptable.
- **`f32` cursor score vs `f64` blend.** Mitigated — the exact `short_id`
  boundary, not the score, decides which rows a page excludes.
- **Blend constants (3 / 1 / 5) are heuristic.** Unchanged from the as-built
  code. The spec captures the formula, not the numbers, so tuning later does
  not break a test contract.
