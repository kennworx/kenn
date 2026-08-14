## Why

`kenn-mcp` is 87% not MCP. Of its 5,761 lines, 733 are the rmcp transport and the
rest is the query layer that answers `find`, `get`, `search`, `list callers`,
`overview`, the findings store, and all five atlas axes. That layer has two
consumers, not one: the MCP server registers 35 of its functions as tools, and the
CLI calls roughly forty of them directly from `cmd_query.rs`.

So `kenn tables` does not reach into an MCP server to answer a question about the
atlas — it calls the same function `find_symbol` comes from. The name says
otherwise, and that misreading has already happened in review: an atlas axis
appearing to live in the MCP crate reads as a layering violation, and explaining
that it is not costs a paragraph every time.

The coupling is thin enough to make the name the only real problem. Measured:

| module | lines | uses `rmcp` |
|---|---|---|
| `tools/` (16 files) | 5,028 | no — one field, read only by `indexing/` |
| `types` `cursor` `error` `result_cache` | 1,671 | no |
| `server/` | 733 | yes — transport, 35 `#[tool]` wrappers |
| `indexing/` | ~900 | yes — progress notifications, roots rebind |

`error.rs` does not import rmcp at all; `json_rpc_code()` returns a plain `i32`.
The single rmcp-typed field on `ServerState`, `peer: OnceLock<Peer<RoleServer>>`,
is written once and read three times, all four sites inside `indexing/`. No query
function touches it.

**The split line is already specified.** `cli-query-surface` states that
`wait_for_index`, `watch_start`, `watch_stop`, and `debug_env` SHALL NOT be
mirrored by the CLI, and that `get_index_status` and `reindex` are covered by
`kenn status` / `kenn index`. That exclusion set is exactly the contents of
`tools/lifecycle.rs`. The tools the CLI does not mirror are the daemon-control
tools, and they are the ones that stay behind. What the CLI mirrors is what moves.

## What Changes

- Extract a **`kenn-query`** crate holding the transport-agnostic query layer:
  `tools/` minus `lifecycle.rs`, plus `types.rs`, `cursor.rs`, `result_cache.rs`,
  and the error type. `kenn-mcp` keeps `server/`, `indexing/`, `watcher.rs`,
  `state.rs`, and `tools/lifecycle.rs`, and depends on `kenn-query`.
- Replace `&ServerState` with a **`QueryCtx`** as every query's first argument.
  Queries today open with `state.with_db(|h| …)` and then use only `h.read` and
  `h.snapshot_id`; the lifecycle gate that `with_db` performs is a daemon concern
  and stays in `kenn-mcp`. `QueryCtx` carries the open reader, the snapshot id,
  and the four pieces of context queries actually reach for — `source_root` (6
  sites), `config` (2), and the embedder stage/error pair (5).
- Rename `McpError` to **`QueryError`**, moving `json_rpc_code()` out to the
  transport crate. The variants are query-domain facts (`StaleCursor`,
  `EmptySnapshot`, `EmbedderStarting`); the JSON-RPC numbering is what MCP does
  with them.
- Move `peer` off `ServerState` into `indexing/`, its only reader.
- **No behavior change.** Same tool set, same wire shapes, same CLI output, same
  error codes on the wire. The existing suites are the guard: any diff in what a
  tool returns is a defect, not an intended consequence.

Once queries are pure over a snapshot, exposing an axis through MCP stops being a
layering question and becomes a six-line wrapper in the transport crate — so this
change closes `atlas-tables` 3.5, which stalled on exactly that ambiguity.

## What This Does Not Buy

It does **not** shed `rmcp` from the CLI binary. `kenn-cli` ships `kenn server`
(`cmd_mcp.rs`), so the transport is linked either way. Anyone evaluating this on
binary size or dependency count will find nothing.

What it buys is narrower and real: a query test constructs a reader instead of an
`Arc<ServerState>`; editing `server/core.rs` recompiles 733 lines instead of
5,761; and the crate name stops contradicting its contents.

## Capabilities

### Added Capabilities

- `query-layer` — the transport-agnostic query crate: what it answers, what it may
  depend on, and what it must not.

### Modified Capabilities

- `cli-query-surface` — the mirroring requirement names `kenn_query::*` rather
  than `kenn_mcp::tools::*`, and states the shared-implementation rule that
  mirroring depends on.
