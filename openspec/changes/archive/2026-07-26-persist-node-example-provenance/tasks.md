## 1. Carry the fact on the node

- [x] 1.1 Add `example: bool` to `AggregateNodeRecord` (`kenn-model`), `#[serde(default)]` like its `test`/`external` siblings.
- [x] 1.2 Add the `example` column to the `aggregate_nodes` DDL, the `AggregateNodeRow` type, the reader `SELECT`, and the writer `INSERT`.
- [x] 1.3 Bump `STORE_SCHEMA_VERSION` 3 → 4 so an old snapshot reports a mismatch instead of failing at the `SELECT`.

## 2. Evaluate it once, where the inputs already are

- [x] 2.1 Move `EXAMPLE_SEGMENTS` + `is_example_path` from `atlas/producer.rs` to `aggregate.rs` as `pub(crate)`; repoint the sub-area caller that still needs the path form.
- [x] 2.2 Pass `files` + `primary_def_file` (already built for `resolve_anchors`) into `build_aggregate_nodes` and set `example` there.
- [x] 2.3 Test: a node whose primary def is under `examples/` is flagged, one under `src/` is not — mutation-checked by neutering the predicate and watching the flag go false.

## 3. Make both surfaces read it

- [x] 3.1 `atlas/producer.rs`: node eligibility reads `n.example` instead of joining `primary_def_file` → `files`.
- [x] 3.2 `kenn-mcp/tools/domains.rs`: pass `example: n.example` instead of `false`; delete the `KNOWN GAP` module comment.
- [x] 3.3 Test: a community whose entire second-package span is example code yields no domain from EITHER surface — the producer test and the query test assert the same absence.

  Mutation-checking the producer half found the first attempt guarded nothing:
  the producer excludes example nodes TWICE (an early `continue`, and the
  `example` fact passed to `is_domain_eligible`), so neutering either alone left
  the other standing. Only the conjunction fails the test; the doc comment now
  records that instead of the false single-site claim.

## 4. Verify on the repo that exposed it

- [x] 4.1 Reindex; confirm `kenn domains` and the rendered atlas report the same count. — 11 == 11, and `diff` of the two title sets is empty.
- [x] 4.2 Confirm the specific domain carried by `crates/kenn-store/examples/` is the one that disappears — a matching count with a different membership is not a fix. — `LlamaEmbedder` absent from both; all four culprit nodes carry `example=1` in the snapshot (59 flagged of 5278).
- [x] 4.3 Confirm `kenn packages` / `kenn contracts` / `kenn documents` are unchanged: this touches domain eligibility only. — 14/14, 1/1, 4/4 against the atlas.
- [x] 4.4 Verify on ONE REAL REPO PER LANGUAGE, not just this workspace — the flag is derived from a path convention, but the def→file join it rides on is per-indexer. Where the resident repo had no example tree, clone one that does; a null case proves only that nothing broke.

  | lang | repo | atlas/query | flagged | example files |
  |---|---|---|---|---|
  | Rust | this workspace | 11 / 11 ✓ | 59 / 5278 | 12 |
  | Go | afero | 2 / 2 ✓ | 0 / 389 | 0 |
  | Go | go-plugin | 2 / 2 ✓ | 92 / 735 | 31 |
  | TypeScript | swr | 8 / 8 ✓ | 17 / 374 | 38 |
  | Python | httpx | 20 / 20 ✓ | 0 / 1250 | 0 |
  | Python | flask | 13 / 13 ✓ | 55 / 1518 | 30 |
  | Swift | swift-argument-parser | 17 / 17 ✓ | 17 / 1534 | 6 |
  | C# | a 125-package solution | 24 / 78 — render cap | 0 / 18419 | 0 |

  Every flagged node in every repo resolves to a path with an example segment.

  Two things the sweep settled that this workspace could not:
  - **C#'s zero is correct, not a gap.** Its five `%sample%`/`%fixture%` files
    carry the word in the FILENAME (`Samples.cs`, `AspNetFixture.cs`), and
    `is_example_path` matches whole SEGMENTS — so a substring rule would have
    wrongly excluded five production types from the domain axis. The repo's real
    `examples/*.cs` live in vendored projects the solution never references, so
    they are not indexed at all.
  - **Swift's names are not filename-safe.** `replacing(_:with:)` is stored as
    `replacing-_-with.md`, so comparing atlas FILENAMES to query TITLES reports a
    false divergence on Swift alone. Compare the `title:` front-matter field.

- [x] 4.5 PRE-EXISTING, found by the C# run, FIXED in a follow-up commit
  (`db80873`), not in this change: silent truncation of the domains axis. The
  atlas summary read `125 packages · 24 domains` for a repo with 78 —
  `MAX_DOMAINS` capped the render and nothing said so. Directive `fnd_9d77a017`
  requires the opposite ("applies to any bounded atlas list, not just coupling:
  state what was dropped"); coupling had named its cap since the coupling work,
  the two axes never did. Invisible below 24 domains, which is why it survived.
  Contracts turned out to hide more: 126 capped to 24. Both axes now count before
  truncating and the heading names the cap when it binds.

## 5. Gates

- [x] 5.1 `cargo clippy --workspace --all-targets` clean (pedantic).
- [x] 5.2 `cargo test --workspace` green.
- [x] 5.3 `just crap-ci` passes. — "no regressions, no new over-threshold functions".
- [x] 5.4 `cargo fmt --all` last. — touched only files this change edited; no drift to split out.
