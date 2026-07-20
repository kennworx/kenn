## Context

A code audit + a live thread-sample of a running `kenn mcp` server (during
a reported hang) established two things:

1. The server itself was **not** deadlocked in the observed incident — it
   was idle, blocked only on the normal stdin read. That stall was
   client/transport-side and is out of scope here.
2. The audit found handler-level hazards that *would* stall the server
   under concurrency, violating the invariant ("every tool < ~200ms except
   `wait_for_index`; never block another tool indefinitely").

This change fixes the concurrency + boundedness + observability hazards.
The separate synchronous-SQLite-on-the-runtime hazard is handled in
`mcp-offload-blocking-storage` (it reopens an architecture decision and
was not an observed cause).

## Goals / Non-Goals

**Goals:**
- Concurrent read tools (especially findings) run in parallel — no
  exclusive lock on a read-only path.
- `search_findings` cost is bounded by the limit, not the corpus size.
- Every tool call is logged with name + duration.

**Non-Goals:**
- Offloading synchronous SQLite off the runtime (separate change).
- The client/transport stall (server was idle).
- `get_source` bodies, new search/grep tools.

## Decisions

### D1: Findings store `Mutex` → `RwLock`; reads take `.read()`

The hazard is that `with_findings` acquires `tokio::Mutex<Option<FindingsStore>>`
(exclusive) and holds it across the closure's `.await` — so every findings
tool serializes. But the read methods are already `&self`
(`search_findings(&self, …)`, `all_findings(&self)`, the DAG walks,
`get_finding`); only `store_finding` / `merge_findings` / `record_anchor`
mutate. So reads never needed exclusive access.

Switch the field to `tokio::sync::RwLock<Option<FindingsStore>>`. Split
`with_findings` into:
- `with_findings_read` — takes `.read().await`, hands the closure
  `&FindingsStore`; used by `search_findings`, `get_finding`,
  `find_predecessors`, `find_successors`, `find_directives`, `check_anchors`.
- `with_findings_write` — takes `.write().await`, hands `&mut`; used by
  `store_finding`, `merge_findings`, `record_anchor`.

Concurrent reads then proceed in parallel; writers still serialize (and are
brief). This is the **primary** fix for the observed reader-reader stall.

*Verification step:* confirm each tool's underlying store method receiver
(`&self` vs `&mut self`) before routing it to read vs write — the split is
only correct if the read methods truly don't mutate.

### D2: Bound `search_findings`

`search_findings` currently `all_findings()` then scores every finding.
Push the limit into the lexical (BM25/FTS5) stage so the candidate set is
capped, then vector-score only that bounded set. Deterministic top-K;
cost O(limit), not O(corpus). Complements D1 — even the write path never
holds the lock across an unbounded scan.

### D3: Log tool name + duration — injection point is a spike

`json_result(r: Result<T, …>)` receives only the result — it has **no tool
name and no start time**, so it cannot be the logging point (an earlier
draft assumed it could). Dispatch is the macro-generated `#[tool_handler]`.
Two viable injection points, to be settled by a short spike:
1. A per-`#[tool]`-method timing wrapper (name known at the call site).
2. A custom `call_tool` that wraps the generated router with a timing span.

Prefer whichever rmcp supports without forking the macro. The requirement
is name + elapsed per call; the mechanism is the spike's output.

## Risks / Trade-offs

- [`RwLock` write-starvation if reads are constant] → reads are short
  (bounded by D2) and findings writes are infrequent; tokio's `RwLock` is
  write-preferring enough in practice. Revisit only if writers starve.
- [Bounding `search_findings` changes results for very large stores] →
  deterministic top-K is the intended contract; full-corpus scoring was
  never specified.
- [Logging every call adds noise] → one line per call at a grep-friendly
  level, gated by log level.

## Open Questions

- The exact logging injection point (D3) — resolved by the spike during
  implementation; does not change the requirement.
