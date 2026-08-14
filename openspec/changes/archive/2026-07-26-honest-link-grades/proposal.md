## Why

`kenn check links` reports six false problems out of seven on kenn's own repo.
Five well-formed links — `` [`docs/`](docs/) ``, `[MIT](LICENSE-MIT)`,
`` [`kenn-model`](../kenn-model) `` and friends — are reported **dangling**
(the spec's word is "broken") while pointing at things that plainly exist. One
correct relative link to a source file is reported **drifted**. Exactly one row
is real.

Two faults produce that:

1. **A relative-path resolution bug.** `code_resolve::normalize` "resolved" `..`
   by *deleting the token* (`path.replace("../", "")`) instead of popping a
   segment. It was the only one of the repo's three relative-join
   implementations that got this wrong, and it was the one on the shared grading
   path for both md→code and HTML→file links. The bug cut both ways: a correct
   link missed its exact match and landed on `drifted`, and — because candidates
   are pre-filtered by basename — a link could be graded **`exact` against the
   wrong same-named file**. This repo has 38 `mod.rs`, 11 `tests.rs`, 10
   `lib.rs`. A false `exact` is a wrong answer the report does not flag.

2. **Half of the attachment model was never built for markdown.** HTML already
   resolves a reference to an existing-but-unindexed file: `AssetIndex::exists`
   checks the workspace, and `mint_asset` mints an `attachment` stub keyed by the
   **canonical workspace-relative path** — so every spelling of one target
   collapses to one node — and grades the edge `Exact`. `html-index` describes
   this as "reusing the markdown attachment model", but markdown has only the
   attachment *kind*: it mints a stub keyed by the written string and grades it
   `Dangling`, with no existence check at all. And HTML's own gate
   (`is_asset_ref`) requires a non-indexed *extension*, so an extensionless file
   or a directory falls out of the model on both sides. Those are exactly the
   five false positives.

Why now: CLAUDE.md §10 directs agents to prefer the graph over `rg` because
*"grep cannot prove absence — the graph can."* Here the graph asserts absence of
five things that are present, in the first file a new reader opens.

## What Changes

- **Share the relative-path join.** One `join_relative` in
  `crates/kenn-indexer/src/relpath.rs`, used by md↔md, md→code, and HTML.
  `code_resolve::normalize` and `html::links::core::canonical_path` are deleted.
  The md→code exact rung now mirrors md↔md: the path **as written** or the path
  **joined** onto the linking file's directory both grade `Exact`.
- **Finish the attachment model on the markdown side.** Give markdown the same
  existence oracle HTML has. An unresolved markdown target that exists in the
  workspace becomes an `attachment` stub keyed by its canonical
  workspace-relative path, graded `Exact` — identical to what HTML already does
  for `<img src="logo.png">`. A target that does not exist keeps today's
  written-string stub and `Dangling`.
- **Let existence decide eligibility, not spelling.** `is_asset_ref` answered
  `false` for a name with no extension, routing `LICENSE-MIT` to the
  document/symbol branch and then to `Dangling`. The gate is deleted outright:
  `mint_asset` already dangles a target the workspace does not hold, so it only
  ever suppressed *existing* targets — including an excluded `.md`, which HTML
  dangled while markdown resolved it. The filesystem backing widens from
  `is_file()` to `exists()` so a directory resolves, and one shared
  `existing_target_kind` decides `Document` vs `Attachment` on both sides.
- **Prefer an existing path over a same-named symbol, for inline links.** A bare
  inline destination (`[the docs](docs)`) denotes a path in `CommonMark`, so it
  must not be shadowed by a `fn docs`. A wikilink keeps symbol-first, and a
  path-shaped target still resolves to its indexed file node.
- **Close the spec gap that caused this.** `markdown-link-graph` specifies only
  the *stale* relative-path case — no scenario for a correct relative path being
  `exact`, none for a target that exists but is not indexed. Add them.

Explicitly **not** changing: no new `LinkGrade` variant, and no change to the
`check_links` default filter or its `total` semantics. HTML's asset *grading*
and stub ids are untouched; its eligibility gate is not (see What Changes). The
five false positives stop being reported because they genuinely resolve, not
because the report hides them.

## Capabilities

### New Capabilities

None — this corrects and completes existing behavior.

### Modified Capabilities

- `markdown-link-graph`: the resolution ladder gains a correct-relative-path
  scenario and the existence-backed attachment resolution markdown was missing.
- `html-index`: HTML link references already delegate to the markdown resolver's
  file resolution and grading, so they inherit the relative-path fix; the asset
  eligibility gate is replaced by an existence check, and the resolved node's
  kind now comes from the shared markdown rule.

## Impact

- `crates/kenn-indexer/src/relpath.rs` — new; holds the single join rule, the
  `PathExists` seam (was HTML's `AssetIndex`), and its one filesystem backing.
- `crates/kenn-indexer/src/markdown/resolve.rs` — `join_relative` moved out.
- `crates/kenn-indexer/src/markdown/code_resolve.rs` — `normalize` deleted;
  `resolve_file_ref` takes the shared join, as-written *or* joined.
- `crates/kenn-indexer/src/markdown/ingest/core.rs` — existence-backed
  attachment stubs, keyed by canonical path.
- `crates/kenn-indexer/src/html/links/core.rs` — `canonical_path` and
  `is_asset_ref` both deleted; stub records share their name derivation and take
  their kind from `markdown::existing_target_kind`.
- `crates/kenn-indexer/src/html/ingest.rs` — drops its own `FsAssets`; the one
  `PathExists` backing lives in `relpath.rs` and widens to `exists()`.
- No `kenn-model` or `kenn-store` change: no new grade, no new discriminant, no
  schema change.
- Index format: a re-index re-grades existing edges, the standing rule
  (`kenn index --force`).

Success criterion, checkable today: `kenn check links` on this workspace reports
**1** genuinely-dangling link, not 7 — and `../frames.ts` is graded `exact`.
