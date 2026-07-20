## Why

`search_symbols` is shaped as a paginated tool but implemented as a
top-K relevance probe. The two contracts collide in ways that mislead
the agent:

1. **`total` reports the over-fetch pool, not corpus matches.** In
   `DbReader::search_symbols_blended` (`crates/kenn-store/src/db/reader.rs:407`)
   the underlying Lance probes use
   `pool = max(8·limit, 64)`, and `total` is computed as the
   de-duplicated union of three top-`pool` hit-sets. So the same
   query returns wildly different `total` values depending on the
   `limit` the agent passed:

   | limit | items | `total` |
   |---:|---:|---:|
   | 1   | 1   | 67   |
   | 3   | 3   | 67   |
   | 10  | 10  | 81   |
   | 25  | 25  | 180  |
   | 50  | 50  | 350  |
   | 100 | 100 | 715  |
   | 200 | 200 | 1428 |

   An agent that uses `total` to decide "should I paginate?" or
   "show user N matches" gets unstable garbage.

2. **Pagination cannot reach beyond the pool.** `merged` is capped at
   `pool` results (line 451). Once the cursor walks past `pool`
   entries, `next` becomes `None` and the agent thinks the stream is
   exhausted — even though corpus matches outside the top-`pool`
   exist. With `limit=10` (`pool=80`), at most 8 paginated pages are
   reachable.

3. **The conceptual mismatch.** Today's `limit` is a page-size hint;
   the agent has to walk pages to assemble a result set. For a top-K
   relevance tool, the natural shape is "server picks K, agent picks
   how many to read per response." For iteration tools, the agent
   genuinely wants to walk all callers — capping the corpus is wrong.
   Today's one-knob name (`limit`) papers over two very different
   contracts.

Recast every paginated kenn tool around a single agent-facing knob:
**`page_size`** — how many rows the agent wants per response. The
server picks the top-K materialize cap (fixed at 30) and provides
family-specific defaults; the agent overrides when it knows what it
needs.

- **Top-K tools** (`search_symbols`, `search_findings`,
  `semantic_search`): server always materializes top 30; agent
  paginates within them via `page_size` (default 10, max 30).
- **Iteration tools** (`list_*`, `find_*`): no server-side total cap;
  agent walks the corpus via `page_size` (default 25, max 50).

The `total` field is dropped from every paginated response — its
previous meaning was leakage of internal over-fetch state and was
useful to no one. The `limit` parameter and `count_only` flag are
removed.

Note: MCP's pagination spec (`server/utilities/pagination`) defines
opaque cursors for the four meta-operations (`resources/list`,
`prompts/list`, `tools/list`, `resources/templates/list`) but says
nothing about `page_size` parameters or pagination on tool *results*
(`tools/call`). Kenn's pagination is a kenn-specific extension — the
agent's only contract is what this change writes into each tool's
description string and the kenn skill.

## What Changes

### `search_symbols` becomes a cache-backed top-K tool

- `SearchSymbolsArgs.pagination` keeps `{ page_size, cursor }`; the
  `limit` field is removed. `count_only` is removed.
- Response shape: `{ items, next }` — `total` removed.
- First-call behaviour: reader materializes top
  `TOP_K_MATERIALIZE = 30` rows once; tool returns
  `clamp_top_k_page(page_size)` rows, stashes the rest in
  `ResultCache<RankedSymbolRef>`, emits a cursor.
- Continuation: cache slice, emit cursor until exhausted.
- Tool description: "`page_size` is rows per response (default 10,
  max 30 — server materializes top 30 ranked results; cursor walks
  within them)."

### `search_findings` gets the same shape

- Today single-shot. Adopts the same `{ page_size, cursor }`
  pagination and `ResultCache<RankedFindingView>`.
- `total` dropped. `count_only` dropped.

### `semantic_search` keeps its shape

Already single-shot, no pagination, no `total`. Default and max
`page_size` align with top-K (10/30) — but no cursor emission.

### Iteration tools (`list_*`, `find_*`): no total cap

- `pagination` argument keeps `{ page_size, cursor }`; `limit` and
  `count_only` removed.
- `page_size` default 25, max 50.
- Cursor walks the full corpus until exhaustion (`next: null`).
- Tool description: "`page_size` is rows per response (default 25,
  max 50); cursor walks the full corpus until exhaustion."

### Pool sizing — tighten the over-fetch

Replace `pool = max(8·limit, 64)` with
`pool = min(2·limit, 256).max(64)` in `search_symbols_blended`. At
the new fixed-30 materialize cap, the floor binds → pool = 64
always. Ceiling kept as safety against future cap increases. See
design D7.

### Out of scope

- Changing the blended-ranking algorithm (NAME_BM25_WEIGHT,
  VECTOR_WEIGHT, substring bonus). Separate concern.
- Adding a true "iterate all matches by relevance" mode for top-K.
  Past the top 30, BM25+vector scores are noise; if a use case
  emerges, design it as a separate `scan_symbols` tool.
- Token-cost optimisation for the cache (already bounded by N=64
  entries × ~6 KB).

## Capabilities

### Modified Capabilities

- `mcp-symbol-search`: `search_symbols` keeps its `pagination`
  argument but the `limit` field is removed; `count_only` is removed;
  the `total` field is removed from the response. `search_symbols`
  becomes cache-backed (D12). `search_findings` gains the same
  cache-backed pagination (was single-shot); `total` and
  `count_only` likewise removed. `semantic_search` remains
  single-shot, only its `page_size` envelope tightens to top-K
  defaults.
- `mcp-server`: `ListResponse<T>` keeps its `{ items, next }` shape;
  the `total` field is dropped server-wide for every paginated tool.
  The `Pagination` struct on every tool args type drops `limit` and
  adds `page_size`. `count_only` is removed from every tool. Top-K
  tools get a new cursor shape `(cache_id, offset)` backed by a
  server-side `ResultCache` (design D8b/D12) — opaque to callers,
  walks past page 1 are O(memcpy). Iteration tools' cursors are
  unchanged. Adds a `ResultCache` module under `kenn-mcp/src/`
  (bounded LRU, evicted on snapshot rotation).
