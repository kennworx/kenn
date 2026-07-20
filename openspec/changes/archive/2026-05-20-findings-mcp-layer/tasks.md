> **Scope note (2026-05-20).** Applied with two decisions: (1) `semantic_search`
> is **BM25-only** — vector ranking is deferred to the `embedding-producer`
> change; (2) `get_node` / `get_callers` / `get_callees` are **not** added —
> they already exist as `get_symbol` / `list_callers` / `list_callees`. The new
> code-side tools are just `semantic_search` and `get_source`.

## 1. Read tools

- [x] 1.1 Implement `semantic_search` — BM25 over code and/or findings, scopeable to code, findings, or both (vector half deferred).
- [x] 1.2 Implement `get_source` over the code graph. (`get_node`/`get_callers`/`get_callees` reuse the existing `get_symbol`/`list_callers`/`list_callees` tools — not re-added.)
- [x] 1.3 Implement `get_finding` and `search_findings` over the findings store.
- [x] 1.4 Define the JSON tool schemas for all read tools.

## 2. Write and provenance tools

- [x] 2.1 Implement `store_finding(text, parent_ids, tags)` — return `{ id, similar[] }`.
- [x] 2.2 Implement `merge_findings(ids, text, tags)` — synthesize, record inputs as parents.
- [x] 2.3 Implement `find_predecessors` / `find_successors` DAG traversal.
- [x] 2.4 Define the JSON tool schemas for the write and DAG tools.

## 3. Server wiring

- [x] 3.1 Register all new tools on the existing `kenn-mcp` server.
- [x] 3.2 Confirm the new tools inherit the server's lifecycle behavior (fail-fast while not Ready, async dispatch, progress notifications) with no change to existing requirements.
- [x] 3.3 Verify the tool list contains no task-analysis or work-slicing tool (dumb-primitive guarantee).

## 4. System-prompt fragment

- [x] 4.1 Write the system-prompt fragment: search findings before re-investigating; store a finding at a stable conclusion.
- [x] 4.2 Decide fragment packaging (skill, instruction file, or both) and make it installable alongside the server.

## 5. Subagent-as-extractor documentation

- [x] 5.1 Document the dispatch pattern: orient → slice → fan-out → record → synthesize.
- [x] 5.2 State the coordination rule — via the findings store and returned ids, not file passing — and that dispatch is worthwhile only for genuinely independent sub-investigations.

## 6. Verification

- [x] 6.1 Tool tests: each read, write, and DAG tool returns its contracted shape.
- [x] 6.2 Test `store_finding` returns near-duplicates and `merge_findings` records parents.
- [x] 6.3 Test provenance: `find_predecessors` traces a finding to an originating code-graph node.
- [x] 6.4 Run `cargo clippy --workspace --all-targets` to zero warnings.
