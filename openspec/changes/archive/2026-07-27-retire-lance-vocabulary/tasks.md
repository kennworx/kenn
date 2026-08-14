Every rewritten sentence is checked against the implementation before it is
written (design D4). Record the construct each correction rests on — a DDL line,
a layout accessor, a run-dir listing — so a reviewer can re-check it without
re-deriving it.

## 1. Deltas written and verified

- [x] 1.1 `indexing-orchestrator` — prepare / ingest / finalize. Verified:
  `BatchSink::new(writer.clone(), …)` per language ingester in
  `pipeline/api.rs` (so "one append surface per language, no central writer
  thread" still holds), and `DbWriter { inner: Arc<Mutex<SqliteWriter>> }` in
  `db/sqlite/handle.rs` — clones share one **serialized** writer, so concurrent
  appends are ordered rather than resolved by an optimistic-concurrency manifest
  rebase. The requirement title loses "Lance"; the obligation is unchanged.
- [x] 1.2 `store-layout` — the run directory and the coverage requirement.
  Verified against a real run directory (`atlas  code.db  meta.json
  overview.md  report.json  rust.scip  tmp  vector.db` — no `lance/`) and
  against `layout/types.rs`, which has no `lance` accessor and places findings
  at the derived root. The false clause now names `findings.db`; the three true
  clauses are unchanged.
- [x] 1.3 **Caught while writing 1.1:** the orchestrator's "the `live` symlink
  is flipped" is *also* stale — `store-layout` states `live` SHALL be a regular
  UTF-8 text file, explicitly not a symlink, and `.kenn/local/live` is a 25-byte
  regular file containing `runs/2026-07-26T17-40-16Z`. The first draft of the
  delta copied the stale phrasing forward, which is precisely the failure D4
  warns about — correcting a spec by reading a spec. Fixed to "the `live`
  pointer is repointed".

## 2. Remaining deltas

- [x] 2.1 `scip-indexer` — "the `files` Lance dataset" (2 mentions). Verify
  against the `files` table DDL in `db/sqlite/schema.rs` before rewriting.
- [x] 2.2 `mcp-symbol-search` — the match tiers cite a "Lance scalar BTREE
  index" and a "Lance n-gram name index" (3 mentions). The replacement is NOT a
  rename: identifier search is `CREATE VIRTUAL TABLE name_fts USING
  fts5(name_text, tokenize='trigram')` and prose is `doc_fts` (`porter
  unicode61`). Check what the exact-match tier actually queries before naming
  it.
- [x] 2.3 `incremental-embedding` — the "Committed versus derived store layout"
  requirement is corrected (databases, not Lance datasets; findings records
  under `.kenn/findings/` with its database at the derived root). **The
  streaming requirement is deliberately NOT rewritten** — see 2.3a.
- [x] 2.3a **Resolved — split out to `bounded-embed-pass`.** The requirement
  was right and the code had regressed: `run_embed_pass` collected the whole
  pending set. Decision taken was to make the pass chunk by the producer's own
  `batch_size` so the two layers agree, rather than relax the requirement.
  Shipped in 56a2208; the streaming requirement's rewording lives in that
  change's delta, not this one, since this change is doc-only by its Non-Goals.
- [x] 2.4 `embedding-producer` — "the Lance native vector index SHALL be built"
  (1 mention) → `vec0(embedding float[768] distance_metric=cosine)`.

## 3. Capability rename (D3)

- [x] 3.1 Rename `openspec/specs/lance-search/` → `openspec/specs/code-search/`
  and drop the now-satisfied deferral sentence from its Purpose.
- [x] 3.2 Update the two live specs that reference the capability by name
  (`embedding-producer`, and `lance-search`'s own self-reference). Leave the 20
  archived changes alone — they record a capability that genuinely had that
  name.
- [x] 3.3 `openspec validate --strict` passes; 51 capabilities, unchanged (a
  rename, not an addition).

## 4. Verify

- [x] 4.1 Run. Seven hits, five as predicted and two explained:
  - `index-store-db` 273 / 295 / 312 — the prohibition, the "no Lance dataset
    directory" assertion, and "As under Lance, the engine enforces no key
    uniqueness" (a genuine behavioural comparison, verified in context).
  - `code-search` 25 / 51 — the `IVF_PQ` quality bar and the ranking-parity
    gate, both dated.
  - `incremental-embedding` 54 / 89 — **not a miss, but not predicted either.**
    This is the streaming requirement D5 deliberately excluded; its rewrite
    lives in `bounded-embed-pass`'s delta and lands when that change archives.
    The criterion as written did not account for the split it describes three
    sections above, which is a flaw in the criterion, not in the sweep.
- [x] 4.2 Each correction was checked against code as it was written (the
  citations are in tasks 1.1–1.2 and 2.1–2.4), and that discipline caught two
  substantive findings a find-and-replace would have buried: the false
  `runs/{id}/lance/findings/` coverage clause, and the lost embed-pass memory
  bound. It also caught two of my own errors mid-sweep — carrying "the `live`
  symlink is flipped" forward from stale text, and nearly asserting findings
  are lexical-only when `findings/embed.rs` derives their vectors.
- [x] 4.3 Zero `crates/` files touched, as intended — the code was already
  right. (The code fix the sweep uncovered went to `bounded-embed-pass`, which
  is where a code change belongs.)
