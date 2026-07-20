## 1. Audit current cursor emission

- [x] 1.1 Identify every paginated tool in `crates/kenn-mcp/src/tools.rs` and list which ones currently always emit `nextCursor` vs. only-when-more.
- [x] 1.2 Identify every `INVALID_CURSOR` / `STALE_CURSOR` error site; record the current JSON-RPC code and message.

## 2. Spec-align tool-result pagination

- [x] 2.1 Change `INVALID_CURSOR` and `STALE_CURSOR` return paths to emit `-32602 Invalid params`; preserve the kenn-subcode in `data.kenn_subcode` for agent-side disambiguation.
- [x] 2.2 Fix any paginated tool that emits `nextCursor` on the final page; the cursor MUST be `None` when the underlying stream is exhausted.
- [x] 2.3 Document in `crates/kenn-mcp/src/cursor.rs` doc-comments that cursors are opaque, server-controlled, and not durable across snapshots.
- [x] 2.4 Update tool descriptions in `tools.rs` (the `description = "..."` strings) where they reference cursor behavior, so the agent sees the same contract the spec describes.

## 3. Verify `tools/list` pagination contract

- [x] 3.1 Confirm rmcp's `tools/list` handler emits no `nextCursor` when the full list fits — verify in an integration test against the live server.
- [x] 3.2 Add a unit test that asserts `tools/list` response shape: either `{ tools: [...] }` (no cursor) or `{ tools: [...], nextCursor: "..." }`, never an empty-string cursor or a cursor-with-no-more-data.

## 4. Cross-host conformance test

- [x] 4.1 Add an integration test that drives a paginated tool to exhaustion via `tools/call` over the rmcp transport and asserts: (a) every non-final page returns a cursor, (b) the final page does not, (c) feeding the final page's response back produces no further results, (d) a tampered cursor returns `-32602`.

## 5. Verification

- [x] 5.1 `cargo clippy --workspace --all-targets` clean.
- [x] 5.2 `cargo test --workspace` clean.
- [x] 5.3 Manual smoke against Claude Code: invoke `search_symbols` with `limit=5` on a workspace with ≥20 symbols; walk pages until `nextCursor` is absent; verify the agent stops cleanly. — verified 2026-05-24: `cursor pagination` query walked 11 pages (5 items × 10 + final page 2 items), every non-final page emitted `next`, final page emitted `next: null`, walk terminated cleanly.
