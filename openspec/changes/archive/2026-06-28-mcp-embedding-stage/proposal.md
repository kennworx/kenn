## Why

Surfaced dogfooding `audit`/`dup`: on a freshly-indexed repo the MCP **does**
auto-embed in the background (cold start and after every reindex — `spawn_embed_job`
in `orchestrate.rs`), but that pass is **invisible**. `get_index_status` reports
only `indexing` / `ready` / `failed`, with no embedding state, and the server keeps
no embed-pass state at all. So an agent can't tell "vectors still building" from
"vectors will never come," and `find_similar` / the duplication leg fail with no
way to know whether retrying helps.

The fix is to make the background embed a visible pipeline **stage**: extend the
status to `indexing → embedding → ready` (plus `disabled` when no embedder, and the
existing `failed`), and let `find_similar` key its error on it (transient while
`embedding`, terminal otherwise).

## What Changes

- **Unified `state` stage.** `get_index_status.state` gains two values:
  - `embedding` — the code graph is ready and **structural queries already work**;
    only vector queries (`find_similar`, `semantic_search`) are still building.
  - `disabled` — graph ready, no embedder configured; vectors will not be built
    (lexical-only). Structural queries work.

  Progression: `indexing → embedding → ready`, with `disabled` replacing the
  `embedding→ready` arc when no embedder exists, and `failed` for index failure.
- **Server tracks the embed pass.** A new `EmbedStage` (Building / Ready / Disabled)
  atomic on `ServerState`, set by `spawn_embed_job` (Building on spawn; Ready or
  Disabled on completion). `build_index_status` folds it with the lifecycle state to
  produce the reported `state`. `embed_pending`'s `ReembedReport` gains
  `embedder_available` so the MCP can tell `disabled` from `ready`.
- **`find_similar` keys on the stage.** A symbol with no committed vector returns a
  **transient** error (retry — embeddings still building) while `embedding`, and the
  **terminal** `EMBEDDING_UNAVAILABLE` (run `kenn embed` / no embeddable text) once
  `ready` or `disabled`.
- **Contract documented.** Structural queries are available from the `embedding`
  stage onward — an agent that only needs `find_symbol`/`list_callers` must NOT wait
  for `ready`.

## Capabilities

### Modified Capabilities

- `mcp-server`: `get_index_status.state` adds `embedding` and `disabled`, with the
  structural-from-`embedding` contract.
- `mcp-symbol-search`: `find_similar`'s missing-vector error is transient while
  embedding, terminal otherwise.

## Impact

- **No new tools, no schema/migration.** `state` stays a string; two new values.
  `ReembedReport` gains one bool (additive). The MCP already auto-embeds — this only
  makes it observable and makes `find_similar` honest about transience.
