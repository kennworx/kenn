## Why

Agents working a task — fix a bug, add a feature, understand a subsystem — discover facts about the codebase that no AST, call graph, or text index captures: invariants, design rationale, gotchas, why a decision was made, how a flow really behaves. Today that knowledge evaporates when the session ends, so the next task rediscovers it from scratch. A *findings store* persists agent-derived knowledge as first-class, queryable, provenance-tracked records that compound across tasks and sessions. With the committed, merge-clean Lance store from `lance-search-backend` already in place, the findings store is the durable knowledge layer built on the same engine — so a single hybrid search spans both code and accumulated knowledge.

## What Changes

- Add a durable **finding** record to the Lance store: `{ id, text, embedding, tags[], parent_ids[], created_at }`.
- **Free-form `text` and `tags`** — no rigid `kind` enum. Tags are searchable and convention-driven (set by the caller / orchestrator), so the vocabulary can evolve without a schema change.
- **Unified ID space.** `parent_ids` reference either code-graph nodes or other findings — the two share one ID space, forming a **derivation DAG**: every finding traces back through its parents to the evidence it was built from.
- Store operations: `store_finding`, `get_finding`, `search_findings` (hybrid BM25 + vector), `find_predecessors` / `find_successors` (DAG walk), and `merge_findings` (synthesize several findings into one, recording the inputs as parents).
- **Immutability.** Findings are append-only. A correction is a new finding that supersedes the old via `parent_ids` + a `supersedes` tag; a deletion is a tombstone finding. The on-disk findings set is therefore write-once — every git merge is a conflict-free file union, exactly as for `lance-search-backend`.
- **Staleness is computed at read time**, never stored: a finding whose referenced code-graph evidence no longer exists in the current branch is flagged stale in query results. This keeps findings immutable while keeping staleness branch-correct.
- **Lifecycle.** `store_finding` writes to a pending area; findings are merged into the committed store on an explicit flush (default: flush everything the caller did not explicitly drop). Review-before-flush is a later refinement.
- Findings carry their own embedding for semantic search; embedding generation uses the same pluggable producer boundary defined by `lance-search-backend`.

## Capabilities

### New Capabilities
- `findings-store`: durable, committed, merge-clean storage of agent-derived findings — record schema, the unified-ID derivation DAG, hybrid search over findings, DAG traversal, `merge_findings`, append-only immutability with supersede/tombstone semantics, read-time staleness, and the pending→canonical flush lifecycle.

### Modified Capabilities

None. The findings store reuses the `lance-search` capability's Lance dataset, custom `CommitHandler`, merge model, and reconciliation machinery — findings are additional rows, not a new storage mechanism. No existing requirement changes.

## Impact

- **Depends on:** `lance-search-backend` — the committed Lance store, custom `CommitHandler`, merge-clean file layout, and pluggable embedding boundary are prerequisites.
- **Code:** a new findings module in `crates/kenn-store` (record type, store/retrieve, DAG ops, staleness, flush lifecycle); the Lance schema gains the finding row variant (or a sibling dataset); `Reader` / `Writer` surface in `src/api` gains findings methods.
- **On-disk:** findings live in the committed Lance store alongside code search rows; layout version note. No new merge mechanism — the `lance-search-backend` guarantees carry over.
- **Search:** `search_findings` and code search share one engine, enabling a single hybrid query across code and knowledge.
- **Out of scope (follow-ups):** the MCP tool surface and the subagent-as-extractor dispatch pattern (`findings-mcp-layer`); embedding-model selection; review-before-flush UX.
