## Context

`search_symbols` blends three Lance probes (BM25 over names, BM25 over
docs, vector cosine over embeddings) into a single ranked list. The
underlying `DbReader::search_symbols_blended` over-fetches each probe
to a `pool = max(8·limit, 64)` to give the merge step enough
candidates for stable ranking; the agent sees `pool`-derived numbers
through the `total` field and walks pagination cursors that can never
reach past the pool.

This change recasts pagination around a single agent-facing knob —
`page_size` — removes the misleading `total` field, tightens the
over-fetch pool, and pins per-family defaults for top-K relevance vs.
iteration.

## Decisions

### D1: Pagination is controlled by `page_size`, not a total budget

The agent's only pagination input is `pagination.page_size` — how
many rows per response. The server provides defaults but the agent
overrides when it knows what it needs ("I just want the top-1 →
`page_size=1`"). There is no `limit` parameter, no agent-facing
"desired total."

```
                page_size is the only knob

  Top-K tools (search_symbols, search_findings, semantic_search):
    · Server materializes top-K = 30 rows on first call (fixed policy)
    · page_size default = 10
    · page_size max = 30 (the materialized cap; bigger is meaningless)
    · Default behaviour: 3 pages of 10. Agent stops walking when sated.

  Iteration tools (list_*, find_*):
    · No server-side total cap — corpus is the cap
    · page_size default = 25
    · page_size max = 50
    · Cursor walks until corpus exhausts (next: null)
```

Two principles:

1. **Top-K's total is server policy, not agent budget.** 30 is enough
   relevance; past that BM25+vector scores are noise. Agent doesn't
   need to think about "how many top results" — the cap is fixed.
2. **Iteration tools must not cap the corpus.** "Limiting the actual
   list is bad" — the agent has to *see* data to act on it. If the
   agent is hunting through 200 callers for one specific match,
   capping at 100 hides rows it needs.

### D2: Token budget — agent picks the page size that fits its intent

The agent knows its intent better than the server. "Find the top
handler for X" wants `page_size=1`. "Skim top results" wants the
default 10. "Survey all callers" walks pages of 25 until exhaustion.

The server's job is to:
- Default sensibly when the agent omits `page_size`
- Clamp absurd inputs (`page_size=9999` → family max)
- Hand back cursors that the agent walks at its own pace

Cost characteristics:
- **Top-K**: first call materializes 30 rows (pays embedding + 3
  probes once). Subsequent pages are memory slices via the cache
  (D12). Per-page cost: ~constant.
- **Iteration**: per-page work is microseconds of graph scan + sort
  + drain. No cache needed.

### D3: Drop `total` from every paginated response

The current `total` field is misinformation:
- `search_symbols.total` = pool-dependent union (pool=8·limit means
  the same query reports wildly different totals).
- `search_findings.total` = `hits.len()` = identical to
  `items.length`. Tautology.

Drop from `ListResponse<T>` server-wide. Agent reads `items.length`
if it needs that; the `next` cursor signals more-available.

### D4: Response shape — `ListResponse<T> { items, next }`

Continuing existing shape, minus `total`. No new wrapper type. The
field disappears from every paginated tool that emits a
`ListResponse`, not just top-K — its meaning was never agent-useful
for any tool.

### D5: Drop `count_only`

The current `count_only=true` path returns `{ items: [], total }`.
Once `total` is dropped (D3), it returns `{ items: [], next: None }`
— informationally void.

A "boolean-shaped" rescue (`{ found: bool }`) means the same tool
emits two structurally different responses depending on a flag in
the request — complicating the response schema and the agent's
parser for negligible payoff.

Decision: drop the `count_only` argument entirely. Task 1.3's audit
confirmed no internal Rust callers; only tests, which will be
deleted alongside. If any external client relies on it, the same
effect is achieved by calling the tool with `page_size=1` and
checking whether `items` is empty.

### D6: Migration / compat

No internal Rust callers of `search_symbols` exist outside the tool
implementation and its tests. External clients are MCP hosts whose
LLMs consume the tool response as JSON for the prompt — they don't
hard-code the field set. The MCP spec doesn't define tool-result
pagination at all (see `proposal.md` Why), so there's no third-
party "spec deserializer" with rigid expectations.

Operationally low-risk:

- Old `total` field disappears from responses — `Option<T>` parsers
  tolerate it, required-field parsers error. None known.
- Old `limit` field on requests is ignored if the agent passes it —
  the server simply doesn't read it. (Or rejected at deserialize
  time if the args struct uses `deny_unknown_fields`; pick when
  implementing.)

No version bump (project memory: no version bumps while
prototyping). Land the shape change; if a real client breaks,
react.

### D7: Pool sizing — bounded over-fetch (search_symbols_blended only)

This policy is local to `DbReader::search_symbols_blended`. Iteration
tools (`list_callers`, `list_in_scope`, etc.) don't over-fetch — their
cursors walk graph edges by `short_id` order, no pool involved.

The current `pool = max(8·limit, 64)` formula has two flaws:

1. **Unbounded growth.** Today's code is callable with `limit=200`
   (the global `clamp_limit` cap), which produces pool=1600 ×3 probes
   = 4800 candidates per query — most thrown away by the merge
   truncate.
2. **Uncalibrated multiplier.** The 8× factor has no derivation in
   the code or commit history.

Decision: replace with `pool = min(2·limit, 256).max(64)`. With
top-K fixed at `TOP_K_MATERIALIZE = 30`, the floor binds → pool = 64.

| Knob | Reason |
|---|---|
| `2·limit` multiplier | Three probes × 2× = 6× the candidates of any single ranking — enough room for the blended score to promote a candidate that ranks #15 in name-BM25 alone to top-`limit` via vector or doc score. |
| `min(_, 256)` ceiling | Safety guard. At the current `TOP_K_MATERIALIZE=30` the floor binds and the ceiling is dead. Kept so that raising the materialization cap later doesn't reintroduce the linear-in-limit regrowth bug. |
| `max(_, 64)` floor | Binds at all current top-K calls (limit=30 always). Preserves stable recall at small inputs. |

Pool size remains internal — not in the response shape, not in the
kenn-spec contract beyond "MUST be bounded." Documented in the
`search_symbols_blended` docstring as the K-amplification policy.

### D8: Constants and helpers in `types.rs`

```rust
// crates/kenn-mcp/src/types.rs

// Iteration-tool page-size envelope.
pub const DEFAULT_PAGE: u32 = 25;
pub const MAX_PAGE: u32 = 50;

// Top-K page-size envelope. The materialize cap is the absolute
// ranked window; agent never sees more than this in a query.
pub const DEFAULT_TOP_K_PAGE: u32 = 10;
pub const MAX_TOP_K_PAGE: u32 = 30;
pub const TOP_K_MATERIALIZE: u32 = 30;

pub fn clamp_page(page_size: Option<u32>) -> u32 {
    page_size.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}
pub fn clamp_top_k_page(page_size: Option<u32>) -> u32 {
    page_size.unwrap_or(DEFAULT_TOP_K_PAGE).clamp(1, MAX_TOP_K_PAGE)
}
```

No `derive_page_size` — the agent picks; the server only clamps.

### D8b: Two cursor shapes — top-K is cache-backed, iteration is position-based

Top-K and iteration have different cost profiles, so they get
different cursor shapes:

- **Top-K cursor**: `DecodedCursor::TopK { cache_id, offset }`.
  `cache_id` is a random 16-byte UUID minted at first call;
  `offset` is the position into the cached materialized list (≤ 30
  rows). The cache entry knows its own length, so exhaustion is
  detected by the tool layer when the slice would extend past the
  cached list.
- **Iteration cursor**: `DecodedCursor::List { snapshot, last_short_id }`,
  unchanged from today. Iteration's per-page work is microseconds
  of in-memory graph scan + sort + drain — no win from caching.

`cursor.rs` today defines a `Search` variant (`{ snapshot, last_score,
last_short_id }`) that's never emitted. Rename to `TopK` and re-shape
for the cache — same enum slot, new semantics, new wire format. Old
wire format never shipped, no compat concerns.

```rust
// cursor.rs
pub enum DecodedCursor {
    List { snapshot: SnapshotId, last_short_id: u32 },  // iteration
    TopK { cache_id: [u8; 16], offset: u32 },           // top-K
}

// 16 + 4 = 20 bytes raw → 27 base64 chars
pub const TOPK_CURSOR_BYTES: usize = 20;
```

Top-K continuation (in the tool layer, against the cache). The slice
is cloned out of the cache so the mutex guard drops before any
subsequent `.await`:

```rust
let (page, total_len): (Vec<T>, usize) =
    cache.slice(cache_id, offset, page_size)?;
// ^ STALE_CURSOR if cache_id missing
let next = if (offset as usize) + page.len() < total_len {
    Some(encode_topk_cursor(cache_id, offset + page.len() as u32))
} else {
    None  // exhausted
};
```

If the agent passes a different `page_size` mid-walk, it's honored
for that page — re-slicing from the cache at any page size is free.
This is genuinely useful: agent realises page 1 was too narrow,
asks for `page_size=20` on the continuation, gets a wider next
slice.

Backwards compat: cursors aren't durable across sessions, so
in-flight cursors from before the upgrade simply fail to decode
(`-32602`). Agent restarts from page 1 with the new format.

### D9: How agents discover the per-tool envelope

Two surfaces carry the contract:

1. **Each tool's description string** in `tools/list`. Top-K:
   `"page_size is rows per response (default 10, max 30 — server
   materializes top 30 ranked results; cursor walks within them)."`
   Iteration: `"page_size is rows per response (default 25, max 50);
   cursor walks the full corpus until exhaustion."`
2. **The kenn skill** at `claude-plugins/kenn/skills/kenn/SKILL.md`
   — cross-tool wording loaded into the LLM's context before any
   kenn call. Single source of truth for the model.

The schema doesn't carry numeric bounds, so the prose has to.
Per-tool and skill wording MUST agree on the numbers.

### D10: Reader API contract — `search_symbols_blended` takes a fixed materialize limit, called once per query

The reader's `search_symbols_blended(query, limit, cursor_after, ...)`
today re-runs the full top-`pool` ranking on every call and drains
past the cursor. With cache-backed pagination (D12), the reader is
called **exactly once per top-K query** — at first call, to
materialize up to `TOP_K_MATERIALIZE = 30` rows for the cache.
Continuation calls hit the cache and never touch the reader.

Decision: the reader's `limit` parameter becomes `target_total`
(the maximum number of rows to materialize); `cursor_after` is
dropped from the signature entirely. For top-K, callers pass
`TOP_K_MATERIALIZE`. The pool MUST cover `target_total`, i.e.
`pool ≥ target_total`.

Invariant: **`TOP_K_MATERIALIZE ≤ pool_ceiling`**. Currently
30 ≤ 256 ✓. Add a `const _` static assertion next to the constants.

### D11: `search_findings` adopts cache-backed pagination

`search_findings` is today single-shot (`tools.rs:1499-1514`:
`hits = store.search_findings(...)`, returns the full Vec). After
this change it gets the same shape as `search_symbols`:

- Server materializes top 30 (passes `TOP_K_MATERIALIZE` to the
  store)
- Cache holds the resulting Vec
- Agent paginates with `page_size`

Reasons:
1. **Symmetry.** Both top-K relevance tools behave identically;
   the agent has one mental model.
2. **Reuses the cache machinery.** D12 builds `ResultCache<T>`
   parametric over row type. Adding a second cache surface for
   findings is a few lines.

`store.search_findings()` returns a fully-materialized Vec, so the
"first call materialize, then slice" shape (D12) maps cleanly: the
returned Vec is passed straight into the cache as the entry's
`rows`.

### D12: Server-side result cache for top-K queries

Top-K pagination needs the agent's "stop after page 1" affordance
without re-paying the ~30 ms query embedding + 3 Lance probes on
every continuation. Solve by caching the materialized top-K list
in the MCP server process, keyed by a random `cache_id` that rides
in the cursor.

**Shape:**

```rust
// kenn-mcp/src/result_cache.rs (new)
pub struct ResultCache<T> {
    inner: Mutex<LruCache<CacheId, CachedTopK<T>>>,
}

struct CachedTopK<T> {
    /// Defense-in-depth: rotation eviction is supposed to `clear()`
    /// the whole cache before any post-rotation query lands, but if
    /// a query and the rotation hook race, `slice()` can compare the
    /// per-entry snapshot against the current one and emit
    /// `STALE_CURSOR` rather than serve stale rows.
    snapshot: SnapshotId,
    rows: Vec<T>,
}
```

Two concrete instantiations live on `ServerState`:

- `ResultCache<RankedSymbolRef>` — for `search_symbols`
- `ResultCache<RankedFindingView>` — for `search_findings`

`semantic_search` is single-shot (no pagination per proposal.md
"semantic_search is unchanged structurally") so it gets no cache.

**Sizing & eviction:**

- **LRU bound:** `N = 64` entries. ~30 × ~200 B/row = ~6 KB/entry
  → ~400 KB total. Trivial for a dev tool.
- **No TTL.** An agent that asks the user mid-walk may pause for
  minutes; TTL would evict the cache and break the walk.
- **Snapshot rotation evicts everything** via `result_cache.clear()`
  on the rotation hook.

**Cache miss semantics:**

A continuation with an unknown `cache_id` returns `STALE_CURSOR` —
indistinguishable to the agent from a snapshot rotation. In both
cases the right response is "restart the query."

**First-call cost is unchanged.** The cache only amortizes pages
2..N. Whether the cache is populated at all is decided AFTER
materialization, by comparing result length to the agent's page size:

```rust
let rows = reader.search_symbols_blended(&query, TOP_K_MATERIALIZE, ...).await?;
let page_size = clamp_top_k_page(args.pagination.as_ref().and_then(|p| p.page_size)) as usize;
if rows.len() <= page_size {
    // single-shot — fits in one response, no cursor, no cache entry
    return ListResponse { items: rows, next: None };
}
// multi-page — stash and return first slice under a single lock acquisition
let (cache_id, first_page): (CacheId, Vec<_>) =
    cache.put_and_take_first_page(snapshot, rows, page_size);
let next = Some(encode_topk_cursor(cache_id, page_size as u32));
```

The gate `rows.len() <= page_size` covers both single-shot cases
uniformly: agent asked for `page_size=30` (rows ≤ 30, single-shot)
AND corpus produced fewer matches than `page_size` (e.g. agent
defaulted to `page_size=10` but only 4 symbols match — `4 ≤ 10`,
single-shot).

**Concurrency.** The MCP server is one process per workspace.
A `std::sync::Mutex<LruCache>` is fine; lock contention is non-issue
at this call rate. The slice is cloned out of the cache before
releasing the lock — the lock never crosses an `.await`. No async
lock needed.

**Iteration tools do NOT use the cache.** Their per-page cost is
microseconds (graph scan + sort + drain), so caching only adds
memory pressure for no perf win. They continue to emit `List`
cursors as today (D8b).

## Risks

### R1: LLM doesn't internalize `page_size = per-page`

The industry convention (SQL `LIMIT`, REST `?limit=`) is page-size,
so this should be familiar. But LLMs trained on "limit = total"
APIs may pass the field name `limit` instead of `page_size`. Args
struct doesn't accept `limit`; the LLM gets a deserialize error and
self-corrects. Mitigation: clear field name (`page_size`), explicit
description string, skill prose.

### R2: Lost telemetry signal

`total` (even broken) was a coarse signal of "is the corpus
match-rich for this query?". Dropping it removes that signal from
the tool response. Mitigation: server-side `tracing::info!` already
logs query latency; add the pool size to the log line if needed.

### R3: Future iteration-by-relevance use case

Someone wants "list ALL symbols matching 'handler' by relevance".
The right answer is a dedicated `scan_symbols` tool (or
`list_symbols` with a name-prefix filter — already exists via
`find_symbol`'s prefix tier). Don't reopen `search_symbols` for
this; it's a different contract.

### R4: Cursor format break for top-K only

Top-K cursors flip from the `List` encoding (10 bytes: snap + short_id)
to the new `TopK` encoding (20 bytes: cache_id + offset). Any in-flight
top-K cursor from before the upgrade decodes as wrong-length and
returns `-32602`. Iteration cursors are unchanged — those keep working
across the upgrade.

Acceptable because cursors aren't durable across sessions and the
upgrade window is short.

## Open questions

(none remaining)
