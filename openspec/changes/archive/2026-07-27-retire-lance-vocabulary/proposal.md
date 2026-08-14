## Why

Two live specs contradict each other about the storage engine.

`index-store-db` is normative and correct:

> The backend SHALL NOT use Lance, DataFusion, Arrow, redb, or any storage engine
> other than SQLite.

`indexing-orchestrator` is normative and describes a deleted engine:

> ### Requirement: Ingesters write records directly to per-language Lance writers
> … Concurrent ingesters MAY append to the same Lance dataset. **Lance's default
> optimistic-concurrency commit guard SHALL resolve** concurrent appends…

Lance was removed in `replace-lance-with-sqlite` (archived 2026-06-04). A run
directory today holds `code.db`, `vector.db`, `meta.json` — no `lance/`. Yet
**six live capabilities still carry normative requirements written against
Lance**, and one of them asserts a *test-coverage* obligation against a dataset
path that cannot exist:

> the findings store SHALL round-trip writes and reads against a Lance dataset at
> `<derived_root>/runs/{id}/lance/findings/` across an indexer pass
> — `store-layout`, "Deferred runs-centric placements have direct test coverage"

Findings live in `findings.db` at the local root; `.kenn/local/runs/{id}/` has no
`lance/` subtree at all. Three of that requirement's four clauses are true and
verified; the Lance clause is not.

Why it matters here specifically: these are the documents an agent reads to
learn how kenn stores things, and `openspec/specs/` is the promoted, normative
set — not history. A spec that names the wrong engine sends the reader to a
non-existent path, and a spec that contradicts its sibling forces them to guess
which one shipped. This is the same defect class as `honest-link-grades`, one
layer up: the document and the reality disagree, and the reality is fine.

## What Changes

- **Correct the normative Lance references in six live capabilities** to
  describe SQLite as shipped, verified against code rather than against other
  specs: `indexing-orchestrator`, `scip-indexer`, `incremental-embedding`,
  `mcp-symbol-search`, `store-layout`, `embedding-producer`.
  Concretely, the vocabulary maps to what the code actually builds —
  `CREATE VIRTUAL TABLE name_fts USING fts5(tokenize='trigram')` for identifier
  search, `doc_fts` (`porter unicode61`) for prose, and
  `vec0(embedding float[768] distance_metric=cosine)` for exact KNN.
- **Fix the false test-coverage clause** in `store-layout` so it obliges what a
  test can actually assert. The other three clauses of that requirement are
  true and stay.
- **Keep genuinely historical references.** `lance-search`'s "SHALL return at
  least the quality of the prior Lance `IVF_PQ` index" is a real, dated quality
  bar, and `index-store-db`'s "SHALL NOT use Lance…" names it precisely because
  it is forbidden. Neither is stale; both stay.
- **Rename the `lance-search` capability to `code-search`** — the follow-up its
  own Purpose already records ("the capability is still named `lance-search`
  for continuity; renaming it to `code-search` is a separate deferred
  follow-up"). Live references are two specs plus the directory; archived
  changes are historical records and are **not** rewritten.

No code changes. No behavior changes. The code is already right — this is the
documentation catching up, and nothing here should alter a single test.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `indexing-orchestrator`: prepare/finalize/ingest requirements describe SQLite
  writers and the snapshot database, not per-language Lance datasets or Lance's
  optimistic-concurrency commit guard.
- `scip-indexer`: the `files` "Lance dataset" is the `files` table.
- `incremental-embedding`: the embedding job streams a SQLite scan; derived
  artifacts are the per-run databases, not Lance datasets plus `IVF_PQ`.
- `mcp-symbol-search`: match tiers cite the FTS5 trigram index and the scalar
  columns, not a Lance BTREE / n-gram index.
- `store-layout`: the run directory holds `code.db` / `vector.db`, not a
  `lance/` subtree; the findings coverage clause names `findings.db`.
- `embedding-producer`: the vector index is `sqlite-vec` `vec0`, not a Lance
  native index.
- `lance-search`: renamed to `code-search`, its Purpose losing the
  now-satisfied deferral note.

## Impact

- `openspec/specs/` only — six spec files edited, one directory renamed.
- `openspec/changes/archive/**` is deliberately untouched: an archived change
  correctly records the state at the time it shipped, and rewriting history to
  match the present is how a changelog stops being evidence.
- No `crates/` change, no test change, no reindex.

Success criterion: no live spec in `openspec/specs/` states a normative
requirement in terms of Lance, and `rg '\bLance\b' openspec/specs/` returns only
the two dated historical comparisons plus the `SHALL NOT use Lance` prohibition.
