> **Scope note (2026-05-20).** This change is being applied as the **BM25-only
> subset**. Vector / semantic search depends on an embedding producer that
> `lance-search-backend` left as a non-existent follow-up, so the embedding-
> dependent work is deferred: task 2.2 (semantic near-duplicate check), the
> vector half of task 2.4 (`search_findings` is lexical-only), and task 7.2
> (no embedding-producer boundary exists to reuse). The `embedding` column is
> still defined on the record, left null. The `findings-store` spec's search and
> `store_finding` requirements were amended to match.

## 1. Finding record and schema

- [x] 1.1 Define the `Finding` record type: `id`, `text`, `embedding`, `tags[]`, `parent_ids[]`, `created_at`.
- [x] 1.2 Decide one shared Lance dataset vs a sibling findings dataset (resolves the design Open Question); wire it through the `lance-search-backend` `CommitHandler` and merge model. — sibling dataset at `.kenn/findings/`, reusing `CommittedManifestHandler`.
- [x] 1.3 Define the Lance schema for finding rows; ensure `parent_ids` uses the ID space shared with code-graph nodes.

## 2. Store and retrieve

- [x] 2.1 Implement `store_finding(text, parent_ids, tags)` — persist to the pending area, return `id`.
- [ ] 2.2 DEFERRED (embedding gap): semantic near-duplicate check in `store_finding`.
- [x] 2.3 Implement `get_finding(id)`.
- [x] 2.4 Implement `search_findings` — BM25 over `text`, deterministic ordering (vector half deferred).

## 3. Derivation DAG

- [x] 3.1 Implement `find_predecessors` / `find_successors` walking `parent_ids`.
- [x] 3.2 Implement `merge_findings` — synthesize a new finding recording its inputs as parents (synthesize-and-keep; resolves the design Open Question).
- [x] 3.3 Test transitive provenance: a finding traces through parent findings down to an originating code-graph node, with no cycle.

## 4. Immutability: supersede and tombstone

- [x] 4.1 Implement supersede: a correction is a new finding with `supersedes` tag + the prior `id` in `parent_ids`.
- [x] 4.2 Implement supersede-chain resolution at retrieval — prefer the latest in a chain.
- [x] 4.3 Implement tombstone findings and filter tombstoned ids from normal results.

## 5. Read-time staleness

- [x] 5.1 Implement the evidence-resolution check: batch-test a finding's code-graph `parent_ids` against the current code graph.
- [x] 5.2 Flag stale findings in query results without omitting them and without persisting any flag.
- [x] 5.3 Test branch-correctness: the same finding is stale where its evidence is gone, live where it remains.

## 6. Flush lifecycle

- [x] 6.1 Implement the pending area (representation resolved during implementation per the design Open Question). — an in-memory `Vec<Finding>` buffer on `FindingsStore`.
- [x] 6.2 Implement `flush` — commit pending findings; default policy commits all not explicitly dropped.
- [x] 6.3 Implement `drop` for a pending finding. — method named `drop_pending` (avoids the `Drop` vocabulary).

## 7. Wire into the store API

- [x] 7.1 Add the findings methods to the `Reader` / `Writer` surface in `crates/kenn-store/src/api`. — DELIBERATE DEVIATION: findings are workspace-durable with a lifecycle independent of the per-index-run snapshot, so the standalone `FindingsStore` IS the surface rather than methods on the per-snapshot `Reader` trait. `FindingsStore`, `Finding`, `FindingHit`, `CodeNodeResolver`, `RedbCodeNodeResolver`, `finding_is_stale` are exported from `crates/kenn-store/src/lib.rs` (noted in a `lib.rs` comment).
- [ ] 7.2 DEFERRED (embedding gap): no embedding-producer boundary exists to reuse; the `embedding` column stays null.

## 8. Verification

- [x] 8.1 Round-trip and provenance tests (finding store/retrieve, DAG walk).
- [x] 8.2 Merge test: two branches add findings → plain `git merge` → no conflict, union present.
- [x] 8.3 Supersede, tombstone, and staleness tests.
- [x] 8.4 Run `cargo clippy --workspace --all-targets` to zero warnings.
