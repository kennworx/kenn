## Context

The atlas producer computes four axes and renders each to markdown. Three of the
four — domains, contracts, documents — have no query path: their concept types
(`DomainConcept`, `ContractConcept`) are declared in `atlas/model.rs`, built in
`atlas/producer.rs`, rendered by `atlas/okf.rs`, and never leave the indexer.

There is already a precedent for fixing exactly this. `atlas/coupling.rs` was
extracted when the package axis had the same problem, and its module doc states
the constraint this change must honor:

> They start from different inputs — the producer from the in-memory graph
> mid-index, the query from `aggregate_nodes`/`aggregate_edges` rows of a
> published snapshot — but the RULES here … must be one implementation. Two
> copies of a threshold is how a CLI and a document start disagreeing about the
> same repo.

The overview's `cross_anchor_communities: 38` vs the atlas's 9 domains is that
disagreement having already happened, on a counter rather than a threshold.

Two constraints shape the design:

1. **Input asymmetry.** The producer holds `AggregateNodeRecord` /
   `AggregateEdgeRecord` / `SymbolRecord` (indexer-side, mid-index). The query
   holds `AggregateNodeRow` / `AggregateEdgeRow` (store-side, published
   snapshot). Neither can consume the other's types.
2. **Output shape.** kenn's default output is TOON, which renders exactly one
   shape: a flat list of uniformly-typed, non-nested objects. A view type with a
   nested field makes `write_table` error and the whole result fall back to
   pretty JSON. `DomainConcept` (nested `packages`, `central`) and
   `ContractConcept` (doubly-nested `implementers`) are both nested as modelled.

## Goals / Non-Goals

**Goals:**

- Every atlas axis answerable from the CLI/MCP without reading a file.
- One definition per rule: a floor, cap, or ranking exists once and both the
  markdown and the query read it.
- The bare listing of each new verb renders as a TOON table.
- No new tables, no new persisted state, no reclustering on the read path.

**Non-Goals:**

- Persisting the axes as queryable rows (considered and rejected below).
- Changing any axis's *semantics* — the earned-span rule, the is-a weighting,
  the intra-degree hub ranking all stay exactly as they are. This change moves
  where the rules live and adds a second consumer; it does not retune them.
- Byte-identical parity between a rendered `.md` and a query response. The
  markdown is prose-shaped and link-bearing; the query is data. Parity is on the
  NUMBERS and the membership, not the presentation.

## Decisions

### D1 — Share rules via input-agnostic types, not via a common `Record`/`Row` trait

`coupling.rs` does not abstract over its inputs. It accepts already-projected,
neutral structures (`PairWeights<'a> = HashMap<(&'a str, &'a str), HashMap<&'static str, u64>>`
plus `anchor_lang`), and each caller projects its own types into them. The RULES
(`classify`, `Direction::cap`, `couplings`) then operate on the neutral form.

`atlas/domains.rs` and `atlas/contracts.rs` follow the same shape: the shared
module owns the thresholds and the selection/ranking logic over neutral inputs
(anchor names, node ids, weights, per-node metadata), and the two callers each
build those inputs.

**What is shared and what is not.** Only the SELECTION and RANKING are shared —
the floors that decide membership (`MIN_DOMAIN_SIZE`, `MIN_PKG_MEMBERS`,
`MIN_DOMAIN_LINKS`, `MIN_CONTRACT_PKGS`), the earned-span logic, the
intra-degree hub ranking, and the id dedupe. The RENDER caps
(`MAX_CONTRACTS`, `MAX_CONTRACT_PKGS`, `MAX_IMPLEMENTERS_PER_PKG`) stay atlas-side
presentation policy and MUST NOT bound a query — see D8.

This is consistent with the non-goal above: parity is on the numbers and the
membership, not the presentation. Both surfaces agree on WHICH entities qualify;
the atlas then shows a capped top-N with the pre-cap total, while the query pages
through all of them.

*Alternatives considered:*

- **A trait over node/edge sources** (`trait GraphSource { fn nodes(); fn edges(); }`)
  implemented for both Record and Row worlds. Rejected: it forces every rule
  function generic over the trait, spreads lifetime/borrow noise through the
  module, and buys nothing — the projection is a dozen lines per caller and the
  neutral form is already proven by `coupling.rs`.
- **Convert Rows → Records in the query.** Rejected: `Record` types carry
  indexer-internal fields the store doesn't have, so the conversion would
  fabricate values, and a fabricated field is a bug waiting for a reader.

### D2 — Flat view for the listing, nested detail only for a named entity

Each new tool returns a flat row type (`DomainView`, `ContractView`,
`DocumentView`) whose fields are all scalars, so TOON renders the header-once
table. Nested detail is populated only when the caller names a single entity:

```
kenn domains                     kenn domains shared-embedder
─────────────────────────        ──────────────────────────────
id, title, size,                 …the same row, PLUS
packages_count, links            packages[] (spanned, with member
                                 + link counts) and central[]
```

This mirrors `list_packages` exactly (bare = counts; named = full coupling both
directions), and for the identical reason stated in its directive: emitting every
entity's nested detail is quadratic and unreadable at scale.

*Alternative considered:* always return nested detail and accept the JSON
fallback. Rejected — it makes the default output of a listing command JSON,
which defeats the point of TOON being the default, and it scales badly (a
125-package solution has contracts spanning 50+ packages).

### D3 — `id` leads every row

Per the CLI output directive, the resolvable id is the first field of each view
struct, because column order IS struct field order and the id is what the reader
acts on next (`kenn get <id>`, `kenn list implementers <id>`). A contract row's
`symbol` (the interface's own `pub_id`) leads; a domain's `id` leads.

### D4 — `cross_anchor_communities` is relabelled, not silently corrected

The honest fix is not to make the field report 9. The raw count is a real
clustering counter recorded by `kenn-analyze` into the build-time `stats` table,
and the graph-analysis spec defines it as such. Changing its value would make the
stats table and the overview disagree — trading one inconsistency for another.

Instead the overview reports both, named for what each is: the raw counter keeps
its name and meaning, and the earned domain count is added beside it. A reader
seeing `38 raw / 9 domains` learns something true about the filtering; a reader
seeing only `38` was misled.

*Alternative considered:* drop the raw counter from the overview entirely.
Rejected — it is a legitimate signal about clustering behavior, and dropping a
field is a harsher break than adding one.

### D5 — Compute on read; do not persist the axes

The axes are derived from data already in the snapshot, which makes staleness
structurally impossible: the answer cannot lag the snapshot it was computed from.
It also means the queries work on a snapshot whose atlas bundle was never written
— the atlas is an `Option<AtlasContext>` in the pipeline, so a query that depended
on the rendered markdown would answer differently depending on whether the atlas
ran. These read the graph and the analysis directly, so they don't.

Cost is NOT yet measured, and one claim must not be assumed: `list_domains` would
be the FIRST read-path consumer of `analysis_flat_communities` /
`analysis_node_membership`. No MCP tool reads them today (the `Reader` API exposes
`scan_analysis_flat_communities` / `scan_analysis_node_membership`, but nothing in
`kenn-mcp` calls them). So this is a new read of two tables of unknown size, not a
free ride on rows already fetched. Contracts are cheaper: they need the aggregate
edges plus symbol lookups, both of which the packages query already reads.

Task 9.3 measures this on a real multi-language repo. If a domains query is slow,
the fallback is the deferred option below, held to the same "measure first" bar the
text-fallback and edge-payload directives set.

*Alternative considered:* persist `atlas_domains` / `atlas_contracts` tables at
index time. Rejected for now — it adds schema, a migration, and a second source
of truth for data that is fully regenerated on every index. Worth revisiting only
if a query is measured slow, which is the same bar the text-fallback and
edge-payload directives set for their deferred work.

### D6 — Documents axis is a listing, not a document store

`kenn documents` lists first-party non-code directories (the `document` concepts:
`openspec`, `docs`, `claude-plugins`) with their file counts and member paths.
It does NOT serve file contents — `kenn get source` and the markdown index
already do that. The verb exists so an agent can discover that these directories
are tracked concepts at all.

It is its OWN verb rather than a `--documents` flag on `kenn packages`, and it is
wired as a subcommand-capable group (the `find` pattern: `sub: Option<…>` with a
bare default) rather than a leaf verb. Two reasons: a `document` is a different
concept type with different fields, so folding it into `packages` would make that
verb's row shape non-uniform — which the flat-table constraint in D2 punishes
directly, degrading `kenn packages` to JSON. And the axis is expected to grow its
own subcommands and flags, which a flag-on-another-verb cannot absorb.

### D7 — An axis entity is named by a QUERY, not by an id

`kenn contracts <arg>` and `kenn domains <arg>` take a query string that may be
either the entity's display title or its resolvable `pub_id`.

The asymmetry with `kenn packages <name>` is real and worth stating: package
anchor names are unique by construction, so a name IS an identifier there. Type
names are not — two packages can each define `IValidator`. The producer already
proves this: `okf::contract_id` slugs the display name and
`dedupe_contract_ids` appends `-2`/`-3` when those slugs collide. So a title is
inherently a query, and the disambiguated `IValidator-2` is a positional tiebreak
a reader cannot interpret without opening the file — it must never be the only
way to address a contract. The `pub_id` (already carried as
`ContractConcept.symbol.pub_id`) is unique by construction, names its namespace,
and is what `kenn get` / `kenn list implementers` already accept.

When a title resolves to N entities, the response SHALL return all N grouped by
resolved target, each tagged with its `pub_id` — not an error, and not a prompt to
retry with a more specific argument. This follows the standing MCP tool-design
directive: surface match-ambiguity in the response, because a second call to
disambiguate is the anti-pattern. It is the same shape `find_usages` uses.

*Alternative considered:* accept both, error on ambiguity with a candidate list.
Rejected — it is a second roundtrip by construction, which the directive names as
the anti-pattern, and the candidate list it would print is exactly the grouped
response it refused to return.

### D8 — Axis queries paginate; they do not inherit the atlas's render caps

The atlas caps what it renders because a markdown page has a reader. A query has
no such limit, and silently returning the top 24 of 60 contracts is exactly the
defect the coupling tables were changed to stop committing: a truncated list that
reads as the whole truth. Measured on a real solution, one interface had 52
implementer packages — more than double `MAX_CONTRACT_PKGS`.

So the axis verbs are paginating subcommands in the sense the CLI spec already
defines: they expose `--page-size`, `--cursor`, and `--all`, and their tools take
`Pagination` like every other listing. Where a response is nonetheless bounded,
it reports the pre-cap total beside the returned rows, so a reader can always see
what was withheld.

*Alternative considered:* reuse `MAX_CONTRACTS` as the query's limit and skip
pagination. Rejected — it makes the CLI disagree with the atlas about the SET (not
just the presentation), and it re-commits the silent-truncation defect on a new
surface.

## Risks / Trade-offs

- **Extraction changes atlas output by accident** → The extraction is
  behavior-preserving by construction (same functions, new home, callers pass
  the same values). Guard: the existing atlas producer tests must pass unchanged,
  and a re-index of kenn's own repo must produce a byte-identical
  `.kenn/atlas/domains/` + `contracts/` set. Diff the bundle before/after.
- **Query and markdown drift anyway** (different projection bugs, same rules) →
  Guard with a parity test that runs both paths against one fixture graph and
  asserts the same domain ids/sizes and contract ids/spans. This is the test the
  package axis lacks and the reason the overview drift went unnoticed.
- **A domain's title is its hub symbol, which the query must rank identically** →
  Hub ranking is intra-domain degree, not global degree (a deliberate, measured
  choice). It moves into the shared module with the rest; ranking is not
  re-derived in the query.
- **Contracts on a large C# solution are wide** (measured: one interface with 52
  implementer packages) → The listing is flat and capped; the per-package
  implementer breakdown appears only for a named contract, and the caps
  (`MAX_CONTRACTS`, `MAX_CONTRACT_PKGS`, `MAX_IMPLEMENTERS_PER_PKG`) come from
  the shared module so the CLI truncates exactly where the markdown does — and,
  like the coupling tables, reports the pre-cap total rather than truncating
  silently.
- **An empty result is a real answer, not a failure** → Rust and Go keep
  abstractions package-local, so their contracts axis is legitimately empty. The
  verbs must return an empty list, never an error, and the docs should say an
  empty contracts axis is itself a signal.

## Migration Plan

1. Extract the rules with no behavior change; verify the atlas bundle is
   byte-identical on a re-index.
2. Add the MCP tools over the extracted rules, with the parity test.
3. Mirror the verbs into the CLI.
4. Extend `PackageView`; add the overview field.

Each step is independently shippable and independently revertible; nothing
depends on a schema migration, so rollback is a code revert.

## Open Questions

Both prior questions are resolved — the documents axis gets its own
subcommand-capable verb (D6) and axis entities are addressed by query rather than
by id (D7).

- When a title query resolves to N entities, does the grouped response stay a
  flat table? N contracts each carrying nested implementers is nested by
  definition, so it falls back to JSON. That is acceptable (it is the named,
  detail-seeking form, not the listing), but if the ambiguous case turns out to be
  common on large C# solutions it may be worth a flat "which did you mean" row set
  — resolvable ids only — as the table, with detail behind the exact `pub_id`.
  Defer until measured on a real solution.
