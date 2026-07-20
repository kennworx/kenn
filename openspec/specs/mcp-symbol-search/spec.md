# mcp-symbol-search Specification

## Purpose
TBD - created by archiving change symbol-search-redesign. Update Purpose after archive.
## Requirements
### Requirement: Two distinct symbol-search tools partition the search space

The MCP server SHALL expose two symbol-search tools whose contracts make their
intended use cases mutually exclusive:

- `find_symbol(name, kind?, page_size?, include_tests?, include_external?)` —
  for the **identifier** case: the agent has a literal name (from a stack
  trace, task description, prior tool output) and wants exact / near-exact
  matches.
- `search_symbols(query, filters?, pagination?)` — for the **intent** case:
  the agent has a natural-language phrase or a loosely-recalled term and wants
  top-ranked relevance over symbol names AND documentation, surfaced as a
  paginated stream over a fixed top-K window.

The MCP descriptions of both tools SHALL state which case each is for and
explicitly steer the agent toward the other tool for the opposite case.
`search_symbols` SHALL NOT attempt exact-tier matching, and `find_symbol` SHALL
NOT rank by BM25 relevance.

`search_symbols` SHALL accept a `pagination` argument carrying optional
`page_size` (rows per response) and optional `cursor`. The response SHALL NOT
contain a `total` field because today's `total` leaks the implementation's
over-fetch pool size and is meaningless to the agent.

#### Scenario: find_symbol covers a stack-trace identifier

- **WHEN** the agent calls `find_symbol(name: "OrderHandler")` with a name
  copied verbatim from a stack trace
- **THEN** the response items MUST include every symbol whose name contains
  `OrderHandler` (e.g. `OrderHandler`, `CancelOrderHandler`, `IOrderHandler`,
  `Foo.Bar.OrderHandler.Method`)
- **AND** the items MUST be ordered by `match_kind` first, then by `len(name)`
  ascending, then by `short_id` ascending

#### Scenario: search_symbols covers a natural-language intent

- **WHEN** the agent calls `search_symbols(query: "user registration")`
- **THEN** the response returns the first `page_size` symbols by blended BM25 +
  vector relevance, ordered by `score DESC, len(name) ASC, short_id ASC`
- **AND** the response MUST NOT contain a `total` field
- **AND** the cumulative result across all pages MUST NOT exceed 30 (the
  server's fixed top-K materialize cap)

### Requirement: find_symbol returns match_kind on every result

Each row of `find_symbol`'s `items` array SHALL carry a `match_kind` field with
one of four values, in this match-tier order:

1. `"exact"` — the symbol's `name` equals the query (case-insensitive),
   resolved via an equality query on the Lance scalar BTREE index over the
   symbol-name column.
2. `"prefix"` — `name` starts with the query and is not exact, resolved via a
   range query on that same BTREE index.
3. `"contains"` — `name` contains the query as a substring (not a prefix),
   surfaced by the Lance n-gram name index.
4. `"fuzzy"` — the Lance n-gram index surfaced the row but `name` does not
   contain the query as a substring (e.g. query `"order handler"` against
   `Foo.Bar.OrderHandler.M`).

`find_symbol` SHALL order results by `match_kind` (in the order above), then by
`len(name)` ascending, then by `short_id` ascending.

#### Scenario: Match-tier ordering on a compound query

- **WHEN** the agent calls `find_symbol(name: "OrderHandler")` and the
  workspace contains `OrderHandler`, `CancelOrderHandler`, and
  `Foo.OrderHandler.M`, plus a documented method `RegisterOrder` whose
  docstring reads "this handles orders"
- **THEN** items[0] MUST be `OrderHandler` with `match_kind: "exact"`
- **AND** the next item MUST be the shortest `*OrderHandler` prefix or
  substring match
- **AND** `RegisterOrder` MUST NOT appear in the response — `find_symbol` is
  not a documentation search

#### Scenario: fuzzy tier matches a token split with no substring

- **WHEN** the agent calls `find_symbol(name: "order handler")` (with a space)
  and the workspace contains `Foo.Bar.OrderHandler.M`
- **THEN** the response MUST include `Foo.Bar.OrderHandler.M` with
  `match_kind: "fuzzy"`

### Requirement: search_symbols ranks by blended name + doc score

The blended symbol search SHALL fuse its retrieval arms by Reciprocal Rank
Fusion (rank-based), not by additive raw scores, and SHALL include the symbol's
`name_lower` identifier signal (exact/prefix/contains) as a fused arm so that one
search covers both exact-identifier and conceptual queries. An exact identifier match SHALL rank
first via an additive exact-name bonus applied on top of the fused score. The
conceptual (semantic) ranking of prose queries SHALL NOT regress relative to the
prior additive fusion.

The blended search SHALL NOT incorporate a graph-proximity arm that re-weights
or displaces already-ranked hits (evaluated and rejected: net-negative on
conceptual queries across corpora).

#### Scenario: exact identifier query ranks the named symbol first

- **GIVEN** a symbol whose `name_lower` equals the query
- **WHEN** the blended search runs for that identifier
- **THEN** that symbol is ranked first
- **AND** blended identifier recall is on par with `find_symbol_tiered`

#### Scenario: conceptual prose query does not regress

- **GIVEN** a prose query with no `name_lower` match
- **WHEN** the blended search runs
- **THEN** the identifier arms contribute nothing and the semantic ranking is
  unchanged from the prior fusion

#### Scenario: rank fusion replaces additive weights

- **WHEN** arms are combined
- **THEN** each arm contributes by reciprocal rank (`w / (K + rank)`)
- **AND** the prior additive `3 / 1 / 8` magnitude weights are no longer used

### Requirement: search_symbols paginates with a stable cursor

`search_symbols` SHALL support cursor pagination. The cursor SHALL encode
`(snapshot_id, last_short_id)` — the existing 14-character list cursor. The
result order `(score DESC, len(name) ASC, short_id ASC)` is a total order, and
the globally-unique `short_id` alone pinpoints the boundary row; a score
component in the cursor could not resolve ties between rows of equal score, so
none is carried.

A page returned for a given cursor SHALL contain only rows that fall strictly
after the cursor's boundary row under `(score DESC, len(name) ASC,
short_id ASC)`. Consecutive pages SHALL therefore neither repeat nor skip a
row. When the cursor's `snapshot_id` no longer matches the active snapshot, the
server SHALL signal a stale cursor and pagination SHALL restart from the first
page.

`find_symbol` SHALL NOT accept a pagination cursor — its result set is small by
design and its ranking is fully determined by `(match_kind, len(name),
short_id)`.

#### Scenario: search_symbols cursor produces a gap-free continuation

- **WHEN** the agent issues `search_symbols(query: "order")` and then passes
  `pagination.cursor = response.next` to fetch the second page
- **THEN** the second page MUST contain only rows ranked strictly after the
  last row of page 1 under the documented blend ordering
- **AND** no row MUST appear in both pages

#### Scenario: Stale cursor restarts pagination

- **WHEN** the agent passes a `pagination.cursor` whose `snapshot_id` does not
  match the active snapshot
- **THEN** the server MUST signal a stale cursor rather than returning rows
  from a mismatched snapshot

### Requirement: find_symbol respects kind and limit constraints

`find_symbol` SHALL accept the standard `kind?` filter (array of `Kind` values)
and a `limit?` parameter (default 20, hard cap 200). A `limit` above the cap
SHALL be clamped to 200, not rejected.

#### Scenario: Kind filter narrows results

- **WHEN** the agent calls `find_symbol(name: "Order", kind: ["class"])`
- **THEN** every item MUST have `kind = "class"`

#### Scenario: limit caps at 200

- **WHEN** the agent calls `find_symbol(name: "Get", limit: 1000)`
- **THEN** the response MUST cap items at 200

### Requirement: BREAKING removal of search_by_intent

The MCP tool `search_by_intent` SHALL NOT be registered. Its documentation-only
search behavior is subsumed by `search_symbols`, whose blend already weighs
docstring relevance. Agents that previously called `search_by_intent(query: $q)`
SHALL be migrated to `search_symbols(query: $q)`.

#### Scenario: search_by_intent no longer registered

- **WHEN** a client issues `tools/list`
- **THEN** the response MUST NOT contain a tool named `search_by_intent`
- **AND** the response MUST contain tools named `find_symbol` and
  `search_symbols`

### Requirement: kenn skill documents the page_size envelopes

The kenn skill at `claude-plugins/kenn/skills/kenn/SKILL.md` SHALL document the
pagination contract so the agent can adapt its calls without inspecting every
tool description individually. The documentation MUST state:

- `page_size` is the agent's rows-per-response choice, NOT a total budget.
- The per-family envelopes:
  iteration tools default page_size 25 / max 50,
  top-K relevance tools default page_size 10 / max 30.
- Top-K tools have a fixed server-side materialize cap of 30 results —
  the cursor walks within those 30, never beyond.
- Iteration tools have no server-side total cap — the cursor walks the full
  corpus until exhaustion.
- `nextCursor: null` signals "no more rows from this query." For top-K it
  means all 30 (or fewer) have been emitted; for iteration it means the
  corpus is exhausted.

#### Scenario: skill documents the envelope contract

- **WHEN** the agent invokes the `kenn` skill before a kenn session
- **THEN** the skill content MUST include a section that names the two
  envelope families with their concrete default and max page_size numbers
- **AND** the skill MUST state that `page_size` controls rows per response,
  not a total budget
- **AND** the skill MUST explain the top-K materialize cap (30) and that
  iteration tools have no such cap

### Requirement: page_size is the only pagination knob

Every paginated tool in `kenn-mcp` SHALL accept the agent's pagination input
as `pagination.page_size: Option<u32>` only. There SHALL NOT be a `limit`
parameter or any other "total budget" knob on the request side. The server
SHALL clamp `page_size` to the family's bounds and apply the family default
when omitted.

Note on the MCP pagination spec: the upstream spec
(`mcp-pagination-spec-alignment`, see archive) covers `tools/list` and the
other meta-operations but is **silent on tool-result pagination** and on
the shape of `limit`/`page_size` parameters. Kenn's `pagination` argument
and cursor envelope are kenn-specific extensions — the contract is defined
by this spec plus each tool's description string, not by the upstream
pagination spec.

The two families and their envelopes:

| Family | Default page_size | Max page_size | Total cap |
|---|---:|---:|---|
| Iteration tools (`list_*`, `find_*`, `find_similar`) | 25 | 50 | none (cursor walks the corpus) |
| Top-K relevance (`search_symbols`, `search_findings`, `semantic_search`) | 10 | 30 | 30 (fixed server-side materialize cap) |

The server SHALL emit `nextCursor: null` when there are no more rows to emit.
For top-K that means the cached materialized window is exhausted; for
iteration that means the corpus is exhausted.

If the agent passes a different `page_size` mid-walk on a top-K cursor, the
server SHALL honor it — re-slicing a cached top-K result at any page_size is
valid.

#### Scenario: page_size at default yields the default page

- **WHEN** the agent calls `search_symbols(query: "x")` with no page_size
- **THEN** the response items count is at most 10
- **AND** when the agent calls `list_callers(id: "rs:foo")` with no page_size
  the response items count is at most 25

#### Scenario: page_size is clamped to the family max

- **WHEN** the agent calls `search_symbols(query: "x", pagination: { page_size: 9999 })`
- **THEN** the response items count is at most 30
- **AND** when the agent calls `list_callers(id: "rs:foo", pagination: { page_size: 9999 })`
  the response items count is at most 50

#### Scenario: agent picks a tight page_size for a focused query

- **WHEN** the agent calls `search_symbols(query: "x", pagination: { page_size: 1 })`
- **THEN** the response contains exactly 1 item (assuming a match exists)
- **AND** a cursor is emitted (because there are more rows in the top-30 window)

#### Scenario: iteration tool walks the full corpus

- **WHEN** the agent calls `list_callers(id: "rs:hot_function")` against a
  symbol with 200 callers, walking the cursor to exhaustion
- **THEN** all 200 callers are emitted across the pages
- **AND** the server MUST NOT cap the cumulative emission below 200

### Requirement: search_symbols bounds the over-fetch pool

The `search_symbols` implementation SHALL bound its internal over-fetch pool
by a fixed ceiling that does NOT scale linearly with the materialize cap. The
over-fetch pool size SHALL NOT be exposed to callers. Per-query cost MUST
therefore be O(1) in the materialize cap past the ceiling. The implementation
MAY over-fetch up to the ceiling to give the merge step enough candidates for
stable ranking.

#### Scenario: pool size does not scale with materialize cap

- **WHEN** the implementation evaluates `search_symbols` for any legal
  materialize cap
- **THEN** the per-probe pool size MUST be at most 256

### Requirement: search_findings is top-K relevance for the findings store

The `search_findings` tool SHALL return ranked findings by BM25 over the
findings text, paginated under the top-K envelope (`page_size` default 10,
max 30; server materialize cap 30). The response SHALL NOT contain a `total`
field.

This requirement aligns `search_findings` with `search_symbols` and
`semantic_search`: all three tools surface top-K relevance results under the
same envelope.

#### Scenario: search_findings paginates under the top-K cap

- **WHEN** the agent calls `search_findings(query: "stale cursor")` with no
  page_size against a store with 200 matching findings
- **THEN** the response is a single page of up to 10 findings (default
  page_size 10)
- **AND** when the agent walks the cursor to exhaustion the cumulative items
  MUST NOT exceed 30 (the server materialize cap)
- **AND** no response MUST contain a `total` field

### Requirement: BM25 search returns file hits alongside symbol hits

The symbol-search tools (`search_symbols`, and the code scope of `semantic_search`) SHALL return file-doc hits interleaved with symbol hits, ranked by the same blended score. The reader SHALL partition hits by kind before hydrating: a file hit (empty `pub_id`) SHALL be resolved from the `files` dataset by its `id`, and a symbol hit (non-empty `pub_id`) through the existing symbol hydration. A file hit's `id` SHALL NOT be resolved against `SYMBOLS` — file and symbol ids are independent id spaces, so blind resolution would return an unrelated symbol.

Every hit SHALL carry a single `kind` discriminant: a file hit's `kind` is the literal `"file"`, a symbol hit's `kind` is its symbol subtype (`"class"`, `"method"`, …). A file hit SHALL carry `kind`, `path`, and `score` only (its extension implies the language). A symbol hit SHALL carry `id`, `kind`, `loc`, `score`, and `test` (the last omitted unless `true`); a null `loc` marks an external symbol, so there is no separate flag.

`find_symbol` (literal-name lookup) SHALL be unchanged and SHALL NOT return file hits — files have no symbol name to match.

#### Scenario: A query matching a file header returns a file hit

- **GIVEN** `src/OrderIntake.cs` has a file-level doc `"Handles order intake validation"` indexed as a doc row
- **WHEN** the agent calls `search_symbols` with `order intake validation`
- **THEN** the results include a file hit for `src/OrderIntake.cs` carrying `path` and `score`
- **AND** the hit is marked as a file via `kind == "file"`

#### Scenario: Symbol hits are unaffected

- **WHEN** a query matches both a symbol doc and a file doc
- **THEN** both appear in the ranked results, each carrying its `kind`, ordered by score

#### Scenario: A file hit does not collide with a same-numbered symbol

- **GIVEN** a file with `id` N (file id space) and an unrelated symbol with `id` N (symbol id space)
- **WHEN** a file-doc row for that file is hit by BM25
- **THEN** it hydrates from the `files` dataset by its `id` and returns the file
- **AND** it does NOT resolve to the symbol that happens to share id N

#### Scenario: find_symbol returns no file hits

- **WHEN** the agent calls `find_symbol` with a literal name
- **THEN** only symbol matches are returned; no file rows appear

### Requirement: Identifier lookup is separator-agnostic

The identifier-lookup path (`find_symbol_tiered`) SHALL match identifiers by
their words independent of casing/separator style (camelCase, PascalCase,
snake_case). It SHALL split identifiers into lowercase words on both the index
and the query side and match them with a word tokenizer (not trigram), so a
multi-word query finds a symbol whether it is named `cancel_order`,
`CancelOrder`, or `cancel-order`. This word-split matching SHALL NOT be fused
into the blended conceptual search (it regresses conceptual ranking — see
design); blended's only identifier signal is the `name_lower` exact/prefix/
contains fold-in.

#### Scenario: snake_case symbol found by its words via identifier lookup

- **GIVEN** a symbol named `search_symbols_blended`
- **WHEN** `find_symbol_tiered` is queried with `search symbols blended`
- **THEN** that symbol is returned within the top results
- **AND** the same holds for the camelCase form `SearchSymbolsBlended`

#### Scenario: word-split matching does not pollute conceptual search

- **GIVEN** a prose/conceptual query to the blended search
- **THEN** identifier word-token matching does not displace the semantically
  correct result (the word-split arm is not part of blended fusion)

### Requirement: FTS5 queries are normalized through one safe builder

Every FTS5 arm SHALL build its MATCH expression through a single normalizer that
guarantees a valid, injection-safe expression for arbitrary input — including
hyphens, quotes, and operator words (`OR`, `NEAR`, `AND`). The normalizer SHALL
be tokenizer-aware: trigram arms match a quoted literal (substring search); word
arms split the query into tokens and combine them with OR, ranked by BM25 (not
AND).

#### Scenario: query with operator words and punctuation is valid

- **GIVEN** a query containing a hyphen or the word `OR`
- **WHEN** any FTS5 arm runs
- **THEN** the MATCH expression is valid and raises no syntax error
- **AND** the word arm matches by term rather than as one brittle phrase

### Requirement: find_similar signals a missing committed vector

The `find_similar` tool SHALL distinguish a symbol with no committed vector from a
symbol that simply has no near neighbours, and SHALL further distinguish whether a
missing vector is **transient** (the embedding pass is still running) or
**terminal** (it has finished or no embedder exists). When the given symbol has no
committed vector:

- if the server's embedding stage is **building** (the background embed pass is in
  progress — `get_index_status` reports `state: "embedding"`), the tool SHALL return
  a **transient, retryable** error telling the agent embeddings are still building
  and to retry shortly;
- otherwise (the embed pass has finished, or no embedder is configured —
  `state: "ready"` or `"disabled"`), the tool SHALL return the **terminal**
  `EMBEDDING_UNAVAILABLE` error with an actionable message (`kenn embed`, or the
  symbol has no embeddable text).

In neither case SHALL it return an empty result. An empty result SHALL mean only
that the vector exists but no similar symbols were found. This lets an agent running
the `dup`/`audit` duplication leg wait for an in-progress embed pass instead of
mistaking "still building" for "no duplication."

#### Scenario: missing vector while embedding is transient

- **GIVEN** the server's `state` is `"embedding"` (the embed pass is running)
- **WHEN** `find_similar` is called for a symbol with no committed vector yet
- **THEN** it returns a transient, retryable error indicating embeddings are still
  building, not the terminal `EMBEDDING_UNAVAILABLE`

#### Scenario: missing vector after embedding is terminal

- **GIVEN** the server's `state` is `"ready"` or `"disabled"`
- **WHEN** `find_similar` is called for a symbol with no committed vector
- **THEN** it returns the terminal `EMBEDDING_UNAVAILABLE` error naming `kenn embed`

#### Scenario: a vectored symbol with no near neighbours returns empty

- **GIVEN** a symbol that has a committed vector but nothing similar in the corpus
- **WHEN** `find_similar` is called for it
- **THEN** it returns an empty result, not an error

### Requirement: Navigation tools default to excluding test symbols

The graph-navigation tools SHALL default `include_tests` to `false`, matching
the search tools — one universal default across the whole surface. This covers
`list_callers`, `list_callees`, `list_implementers`, `list_overrides`,
`list_usages`, `list_correspondences`, `list_in_scope`, and `list_imports`
(previously `true`), alongside `find_symbol`, `search_symbols`, and
`find_similar` (already `false`). A caller SHALL opt in per call with
`include_tests: true` — for example, to include test callers when scoping a
refactor. `include_external` SHALL likewise default to `false`.
`list_module_files` is exempt: it returns every file and flags `test` /
`external` per row rather than filtering.

#### Scenario: list_callers excludes test callers by default

- **WHEN** `list_callers(id)` runs with no filters
- **THEN** symbols defined in test files are omitted from the callers

#### Scenario: include_tests includes test callers

- **WHEN** `list_callers(id, filters: { include_tests: true })` runs
- **THEN** callers defined in test files are included

