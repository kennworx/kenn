<!-- Groups 2 and 3 were reshaped mid-implementation per user steering:
     "reads use a persisted index/table, never build indexes/tables in the
     read path" and "logging unified with spans + metrics, use the
     observability stack". So group 2 became a persistent findings index
     (not just a bounded query) and group 3 became tracing spans + a
     metrics facade (not log lines). -->

## 1. Findings store: Mutex → RwLock, split read/write

- [x] 1.1 Confirmed receivers: reads (`&self`) = search_findings, get_finding,
      find_predecessors/successors, find_directives, check_anchors; writes
      (`&mut` or mutating `&self`) = store_finding, merge_findings, record_anchor.
- [x] 1.2 `ServerState.findings`: `Mutex` → `RwLock`.
- [x] 1.3 Split `with_findings` into `with_findings_read` (`.read()`, `&store`)
      and `with_findings_write` (`.write()`, `&mut store`).
- [x] 1.4 Routed all 7 findings tools (incl. `semantic_search`'s findings arm)
      to read/write; compiler verified the read sites are non-mutating.

## 2. Persistent findings search index (no read-path build)

- [x] 2.1 New `findings/index.rs`: a persistent FTS5 + lifecycle table at
      `<derived_root>/findings.db` (derived/local, not committed). Built from
      committed records at `open`; maintained per-finding at `flush`
      (supersede/tombstone update target flags via cheap `UPDATE`).
- [x] 2.2 `search_findings` now queries the persistent index: bounded BM25
      (`LIMIT`) with lifecycle filtered in SQL; vector arm scores the persisted
      live rows; only the top-`limit` hits are resolved back to records
      (`record::read_record`) — no `all_findings()` scan, no transient
      `CREATE TABLE` on the read path.
- [x] 2.3 Audited read handlers: `search_findings` was the only read-path
      table build; `find_directives` / DAG walks only scan (no build) — left,
      noted as candidates to move onto the index later.

## 3. Observability: per-tool tracing spans + metrics facade

- [x] 3.1 Custom `ServerHandler::call_tool` (the `#[tool_handler]` macro skips
      its own when one is present) opens one `mcp.tool{tool}` span over all
      tools — single dispatch boundary, no per-method churn.
- [x] 3.2 Span carries tool name + duration (open→close); `metrics` facade
      (`mcp.tool.calls` counter, `mcp.tool.duration_seconds` histogram) at the
      same point; subscriber writes to STDERR with `FmtSpan::CLOSE` (stdout
      reserved for JSON-RPC). No exporter yet — metrics is a no-op until wired.

## 4. Tests

- [x] 4.1 `index.rs` unit tests: rebuild→lexical-search finds by term;
      supersede/tombstone filtered from both `live_records` and lexical search.
- [x] 4.2 Existing `findings_tools` integration tests (ranked hits, paginate
      to cap, merge) pass through the new persistent-index path.
- [x] 4.3 `find_symbol`/`get_workspace_overview` wire contract + full kenn-mcp
      suite green after the RwLock split and the observability `call_tool`.

## 5. Verification

- [x] 5.1 `cargo clippy --workspace --all-targets` clean.
- [x] 5.2 `cargo test -p kenn-store -p kenn-mcp` green.
- [x] 5.3 `just crap-ci` passes.
- [x] 5.4 `cargo fmt --all`.
