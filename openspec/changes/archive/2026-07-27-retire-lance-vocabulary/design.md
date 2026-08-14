## Context

`replace-lance-with-sqlite` (archived 2026-06-04) swapped the storage engine and
updated the specs it was scoped to touch — `index-store-db`, `lance-search`,
`store-layout` in part. It did not sweep the capabilities that merely *mention*
storage while specifying something else, so the vocabulary survived where it was
incidental to the requirement's subject.

Seven weeks on, the audit across `openspec/specs/`:

| capability | Lance mentions | verdict |
|---|---|---|
| `indexing-orchestrator` | 5 | **stale** — one requirement *title* plus the concurrency model |
| `store-layout` | 6 | **stale** — a `lance/` run subtree, and a false coverage clause |
| `incremental-embedding` | 6 | **stale** — "Lance scan", "row groups", `IVF_PQ` rebuild |
| `mcp-symbol-search` | 3 | **stale** — BTREE / n-gram index names |
| `scip-indexer` | 2 | **stale** — "the `files` Lance dataset" |
| `embedding-producer` | 1 | **stale** — "the Lance native vector index" |
| `index-store-db` | 3 | **correct** — names Lance to forbid it |
| `lance-search` | 2 | **correct** — dated quality/parity comparisons |

Ground truth, read from the code and the filesystem rather than from sibling
specs:

```
.kenn/local/runs/2026-07-26T17-40-16Z/   atlas  code.db  meta.json
                                         overview.md  report.json  rust.scip  tmp  vector.db
schema.rs   CREATE VIRTUAL TABLE name_fts USING fts5(name_text, tokenize='trigram')
            CREATE VIRTUAL TABLE doc_fts  USING fts5(doc_text, tokenize='porter unicode61')
            CREATE VIRTUAL TABLE vec_knowledge USING vec0(embedding float[768] …cosine)
layout      findings.db at the local root; no `lance/` accessor exists
```

## Goals / Non-Goals

**Goals:**

- No live spec states a normative requirement in terms of a storage engine the
  project does not use.
- The two live specs that contradict each other stop doing so.
- Every replacement sentence is checked against code, not against another spec.

**Non-Goals:**

- Any code change. The implementation is correct; this is the documentation
  catching up.
- Rewriting archived changes. They are evidence of what shipped when.
- Re-litigating the SQLite decision, which `index-store-db` already records.
- Renaming anything in `crates/` (e.g. the `lance_baseline.json` fixture) — that
  is a code concern, tracked separately if it is worth doing at all.

## Decisions

### D1 — Correct in place; do not delete the requirements

**Decision.** Each stale requirement keeps its subject and its obligation, and
only its storage vocabulary changes. "Ingesters write records directly to
per-language Lance writers" becomes a requirement about per-language writers
appending to the run's snapshot database; the concurrency clause becomes what
SQLite actually gives (a single WAL database with the writer serialization that
implies) rather than Lance's optimistic-concurrency manifest rebase.

**Why not delete them.** The *requirement* — one ingester per language, no
central writer thread, no record channel — is a real design decision (D9) that
still holds. Deleting it to avoid describing storage would lose a constraint the
code depends on. The engine name was incidental to the obligation.

**Why not a blanket find-and-replace.** "Lance dataset" → "SQLite table" is
wrong in at least three of the cases: a *scan* is not a table, `IVF_PQ` has no
SQLite counterpart (the replacement is exact brute-force `vec0`, which is a
different guarantee, not a renamed one), and the "n-gram name index" is now an
FTS5 **trigram** index — a specific tokenizer, not a synonym. Each sentence gets
read and rewritten, or this change just replaces one wrong claim with another.

### D2 — The false coverage clause is narrowed, not dropped

**Decision.** `store-layout`'s "Deferred runs-centric placements have direct
test coverage" keeps its three true clauses (per-language JSONL at
`runs/{id}/{lang}.jsonl`; `findings_local_dir()`/`embed_lock_path()` removed;
`embed-locks/` never created — all three verified) and its findings clause is
rewritten to name `findings.db`.

**Why this requirement is the sharpest one.** It does not merely *describe*
storage — it obliges a **test** to exist that round-trips against
`runs/{id}/lance/findings/`. A coverage requirement pointing at a path that
cannot exist is unsatisfiable: either no such test exists (the requirement is
unmet and nobody noticed) or a test exists that asserts something else (the
requirement is met in name only). Both readings are worse than silence.

### D3 — Rename `lance-search` → `code-search`

**Decision.** Rename the capability directory and update the two live specs that
reference it. The spec's own Purpose already schedules this:

> The capability is still named `lance-search` for continuity; renaming it to
> `code-search` is a separate deferred follow-up.

That deferral was correct when the rename would have been noise during a backend
swap. It is now the last thing keeping a deleted engine's name in the capability
index, where it is the first word a reader sees.

**Archived references stay.** 20 archived change directories contain
`specs/lance-search/`. Those are records of deltas applied at the time, and the
capability *was* called that. Rewriting them would make the archive agree with
the present at the cost of no longer being evidence of the past.

### D4 — Verify against code, one claim at a time

**Decision.** Every rewritten sentence is checked against the implementation
before it is written, and the check is recorded in the task list.

**Why this is called out as a decision.** The failure mode of a documentation
sweep is producing confident new text that is also wrong — and this session has
already produced three of those (a spec asserting cross-corpus node collapse
that the code does not do; a scenario asserting a symbol wins where the code
now prefers a path; a non-goal claiming HTML behavior was unchanged when it
was). A sweep that fixes six specs by reading six specs would repeat it at
scale. The graph and the filesystem are the source of truth here, not prose.

## Risks / Trade-offs

- **Replacing a wrong claim with a differently wrong one.** → D4: each
  replacement cites the code construct it describes (a DDL line, a layout
  accessor, a run-dir listing), and the task list records which.

- **Over-correcting a legitimate historical reference.** The `IVF_PQ` quality
  bar and the `SHALL NOT use Lance` prohibition read like stale mentions to a
  find-and-replace but are load-bearing. → They are listed explicitly in the
  audit table as keep, and the success criterion expects them to survive.

- **A rename churns cross-references.** → Live references are two spec files
  plus the directory; the archive is out of scope by D3. Verified before the
  rename, re-verified after.

- **No test can catch a documentation regression.** There is no gate here — the
  success criterion is a `rg` whose expected output is enumerated in advance, so
  a surviving stale mention is visible rather than argued about.

### D5 — Two findings are NOT vocabulary; they are left for a decision

The sweep surfaced two requirements whose Lance wording hides a substantive
mismatch. Rewriting them to match the code would launder a lost guarantee into
a spec, so neither is included in this change's deltas.

**The embedding pass no longer streams.** `incremental-embedding` requires:

> The job SHALL consume the scan as a stream and embed **one scan batch at a
> time**. Texts and vectors SHALL NOT be accumulated for the whole corpus before
> submission… Peak memory SHALL be bounded by one scan batch plus one in-flight
> producer request, **independent of corpus size**.

`run_embed_pass` does `let pending = scan_rows(&conn, mode)?` and then
`pending.iter().map(|r| r.text.as_str()).collect()`. `scan_rows` returns
`Vec<Unembedded>` for the whole match set with no `LIMIT` — so on
`EmbedMode::Full` the entire corpus's texts are in memory before the first
submission. This is a memory-bounding guarantee that did not survive the SQLite
migration, not a renamed mechanism. Either the code should regain batching or
the requirement should be consciously relaxed; both are decisions, and neither
belongs in a vocabulary sweep.

**`store-layout`'s findings coverage clause.** Corrected here (D2) because the
path it named cannot exist and the intent is unambiguous — but note it was
*unsatisfiable*, which means the test it obliges either does not exist or
asserts something else. Worth confirming a real round-trip test exists once the
wording is right.

## Open Questions

- `crates/kenn-store/tests/fixtures/lance_baseline.json` and its
  `examples/capture_baseline.rs` writer still carry the name, and the fixture's
  own `"backend": "lance"` field records which engine produced the baseline —
  which is *correct* as a historical record. Renaming the file would be churn;
  leaving it is a small ongoing lie in `rg` output. Out of scope here (D-Non-Goals),
  but worth a decision eventually.
