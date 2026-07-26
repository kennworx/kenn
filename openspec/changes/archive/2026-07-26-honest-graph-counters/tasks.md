## 1. Decide where the rule lives

- [x] 1.1 Answer design D2's open question: is `kenn-indexer`'s independence from `kenn-analyze` deliberate? — **Yes, and normative.** `openspec/specs/atlas-bundle/spec.md:71` requires the producer "SHALL NOT ... depend on `kenn-analyze`"; directive `fnd_eb7b643d` states it independently; and `kenn-analyze/src/lib.rs:121` duplicates the `PostAggregateHook` alias explicitly "without the dep on `kenn-indexer`". `post_aggregate_hook` has no readable history (squashed initial commit), so those two are the whole record. The constraint is SYMMETRIC — neither crate may depend on the other — which disqualifies option A outright.
- [x] 1.2 Record the decision in design.md with its rationale, and close the open question. — Done: D2 resolved to a new **option E**, which the original framing missed. The rule does not move at all; `kenn-indexer::aggregate::compute_and_persist` computes the count and writes the row, reading the persisted communities back off the writer's own connection exactly as the atlas already does (and as the spec already blesses). Zero new deps, no crate move, no policy in a types crate, counter present whenever clustering ran.

## 2. Relocate the earned-span rule — NOT NEEDED under option E

- [x] 2.1 ~~Move the shared domain-selection module into the crate chosen in 1.2~~ — moot. `atlas::domains::select_domains` is already extracted and input-agnostic (`atlas-axes-on-the-cli`) and already serves two callers; the new caller is a third in the same crate.
- [x] 2.2 ~~Repoint the atlas producer and the domains query; verify a byte-identical bundle~~ — moot for the same reason: nothing moves, so nothing can drift. The byte-identical-bundle check is retained as task 5.2 against the counter change instead.

## 3. Count at index time

- [x] 3.1 In `compute_and_persist`, after the analysis hook, read back `analysis_flat_communities` + `analysis_node_membership` (the calls the atlas branch already makes) and hoist them out of the `if let Some(ctx) = atlas` branch so the counter is NOT conditional on the atlas — that conditionality is what disqualified option C. — hoisted `scan_analysis_flat_communities` / `scan_analysis_node_membership` above the `if let Some(ctx) = atlas` branch; the atlas branch now reuses them.
- [x] 3.2 Compute the earned count via `atlas::domains::select_domains`, projecting eligibility straight off the aggregate node records with `is_domain_eligible` — reachable without the atlas's `primary_def_file` → `files` joins now that `example` is a persisted node fact. Write `(scope='global', key='', subset='graph', metric='domains')` via `DbWriter::write_stats`. — `earned_domain_count()` in `aggregate.rs`, a pure helper so the orchestrator's branch count does not grow. Eligibility projects straight off `AggregateNodeRecord` including `example`.
- [x] 3.3 Write the row ONLY when clustering produced communities, so "absent" means "analysis did not run" — the same condition under which `cross_anchor_communities` is absent, so the two counters can never be read as disagreeing when one simply isn't there. — guarded on `!flat_communities.is_empty()`.
- [x] 3.4 Test: on a fixture whose raw cross-anchor count exceeds its earned count, both rows are written and they differ — mutation-checked by relaxing a floor and watching only `domains` move. — `earned_count_is_below_the_raw_cross_anchor_count`: raw 2, earned 1 (a single straggler is not a span). Mutation-checked: `MIN_PKG_MEMBERS` 2→1 makes earned 2, i.e. only `domains` moves.

## 4. Report both, named

- [x] 4.1 Add `domains` to `GraphSummary` beside `cross_anchor_communities`; the raw counter keeps its existing name and meaning. — `GraphSummary.domains` beside `cross_anchor_communities`; the raw counter keeps its name and meaning.
- [x] 4.2 Update the tool description / docs so the distinction is stated where a reader meets it — the raw one is a clustering diagnostic, the earned one is the axis. — stated where a reader meets it: `GraphSummary` type docs (incl. the non-nesting correction from D2a) and the atlas SKILL.md orientation step.
- [x] 4.3 Test: the overview reports both, reads them from `stats`, and still performs no aggregation on the read path. — `reshapes_language_manager_and_graph_rows` asserts both, each from its own row. Mutation-checked: dropping the `"domains"` arm leaves it 0. Still one `stats()` read, no aggregation.

## 5. Parity

- [x] 5.1 Assert the `domains` stat equals what the domains query returns for the same snapshot. These two are comparable FOREVER — both read the published snapshot — so this is the durable guard against the original inconsistency recurring in the other direction. — verified on 8 snapshots across 6 languages: stat == query, exactly.
- [x] 5.2 Assert the stat equals the atlas's header total. NOTE the distinction, which is easy to get wrong: compare against the header count, NOT the number of files in `atlas/domains/`. `MAX_DOMAINS` caps the rendered files at 24, so on a large repo the bundle holds 24 documents for 78 domains — the header states the repo's shape and the `## Domains — 78, heaviest 24` heading names the cap (fixed in `db80873`/`35c43c8`). A file-count assertion would fail on exactly the repos that matter. — verified against the atlas HEADER on all 8. The C# repo is the proof the distinction matters: header 78, files 24.
- [x] 5.3 Unlike 5.1, stat-vs-atlas holds only for a bundle rendered by the SAME build: the atlas is a file, a function of the code at index time, while the stat and the query are read from the snapshot. Re-render before comparing, and say so where the check lives — the bundle's own `HEAD <sha>` line is the only staleness signal and nothing reads it. — all 8 repos reindexed with the same build before comparing; the checker fails loudly on a missing value rather than passing empty==empty (it silently did at first).

## 6. Gates

- [x] 6.1 `cargo clippy --workspace --all-targets` clean (pedantic).
- [x] 6.2 `just crap-ci` passes.
- [x] 6.3 Verify per language on the real repos under `./tmp` (see `feedback_no_synthetic_test_repos`), not just this workspace. The large C# solution is the one that matters here: it is the only repo whose raw and earned counts diverge widely AND the only one past `MAX_DOMAINS`, so it exercises 5.2 as well. Small repos pass a counter bug silently. — all six languages; see the table in the commit.
- [x] 6.4 `cargo fmt --all` last.
