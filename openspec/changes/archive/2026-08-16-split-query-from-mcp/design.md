# Design

## The boundary, and why it falls where it does

Three candidate lines were considered. Only one survives contact with the code.

**By file, moving `tools/` wholesale.** Fails: `tools/lifecycle.rs` holds
`get_index_status`, `wait_for_index`, `reindex`, `watch_start`, `watch_stop` —
daemon control that reads `state.lifecycle` (5 sites) and `state.watcher_state`
(2). None of it is a query.

**By "what the CLI uses".** This one is already written down. `cli-query-surface`
excludes `wait_for_index`, `watch_start`, `watch_stop`, `debug_env` from
mirroring, and notes `get_index_status` / `reindex` are covered by `kenn status`
and `kenn index`. That exclusion set is `tools/lifecycle.rs` exactly. The spec
drew this line before the crate did.

**By rmcp reachability.** Agrees with the above, and adds one correction: the
`peer` field on `ServerState` is rmcp-typed but is not a query dependency — one
write and three reads, all inside `indexing/`. It moves with the notification
pump, not with the state.

All three point at the same cut, so:

```
kenn-query                              kenn-mcp  (depends on kenn-query)
─────────────────────────────           ────────────────────────────────
tools/query/  lookup nav usages         server/            transport, 35 #[tool]
tools/  tables contracts packages       indexing/          progress, roots, peer
        domains documents findings      watcher.rs         notify → reindex
        anchors css links semantic      state.rs           lifecycle states
        support tests                   tools/lifecycle.rs status/reindex/watch
types.rs cursor.rs result_cache.rs
error.rs → QueryError
```

## `QueryCtx` — and the gate that splits in half

Every query opens the same way:

```rust
pub async fn list_tables(state: &ServerState, args: &ListTablesArgs) -> … {
    state.with_db(|h| async move { /* uses only h.read, h.snapshot_id */ }).await
}
```

`with_db` does two unrelated things:

1. `ready_view_or_err()` — refuses unless the lifecycle is `Ready`, yielding
   `INDEX_UNAVAILABLE`.
2. an empty-snapshot classification — counts `symbols`, and if the snapshot is
   empty asks `ConfigHint` whether the cause is a disabled language or an
   unindexed workspace, yielding `EMPTY_SNAPSHOT`.

**These belong on opposite sides of the boundary.** `INDEX_UNAVAILABLE` is a fact
about a running daemon; nothing about a snapshot makes it true. `EMPTY_SNAPSHOT`
is a fact about the snapshot and the config, both of which a query already holds —
and it is already a query-domain error variant.

So the gate splits along the crate line rather than being assigned to one side:

```rust
// kenn-query — constructing the context IS the empty-snapshot gate
impl QueryCtx<'_> {
    pub async fn open(read: &Reader, snapshot_id: u64, cfg: &Config, root: &Path)
        -> Result<QueryCtx<'_>, QueryError>;   // may return EMPTY_SNAPSHOT
    pub async fn open_allow_empty(…) -> QueryCtx<'_>;   // overview only
}

// kenn-mcp — the lifecycle gate, then delegate
async fn with_db<R>(&self, f: impl FnOnce(QueryCtx<'_>) -> …) -> Result<R, QueryError> {
    let view = self.ready_view_or_err()?;      // INDEX_UNAVAILABLE
    f(QueryCtx::open(&view.read, view.snapshot_id, &self.config, self.source_root()).await?).await
}
```

The 27 `with_db` call sites keep their shape; what the closure receives changes
from `ReadyView` to `QueryCtx`. `with_db_allow_empty` maps to `open_allow_empty`
the same way.

`QueryCtx` carries the reader, the snapshot id, and only what queries actually
reach for on `ServerState` — counted by name, because most calls wrap as
`state\n    .with_db(` and a receiver-anchored regex undercounts them badly (it
reported 1 `with_db` where there are 21):

| field | sites in `tools/` (minus `lifecycle.rs`) |
|---|---|
| findings store (`with_findings_read`/`_write`) | 24 |
| `source_root` | 6 |
| `search_symbols_cache` / `search_findings_cache` | 2 / 2 |
| `embed_stage` | 2 |
| `config` / `config_present` | 2 |
| `layout` | 1 |
| `embed_error`, `is_stale` | **0** — the first estimate was wrong |

The embedder needs no field: it is process-global, reached through
`tools/support.rs`.

Everything else on `ServerState` — `lifecycle`, `watcher`, `watcher_state`,
`peer`, `model_id` — has **zero** query readers, in production code. The only
appearances outside `lifecycle.rs` are module docs and `tests.rs` setup. That is
the measurement the whole split rests on, so it is worth stating as a count
rather than as a claim.

The payoff is that a query test becomes:

```rust
let ctx = QueryCtx::open_allow_empty(&reader, 1, &Config::default(), root).await;
let got = list_tables(&ctx, &args).await?;
```

No `Arc<ServerState>`, no lifecycle to drive to `Ready`, no server.

## `McpError` → `QueryError`

The type moves whole; only the name and one method change hands. It has no rmcp
import — `json_rpc_code()` returns a plain `i32`, and the wire layer already adds
`data.kenn_subcode` itself.

The variants are query-domain facts. `StaleCursor` is a statement about a cursor
outliving its snapshot; `EmbedderStarting` is a statement about a model still
loading. What MCP contributes is the *numbering*: that both `EmbedderStarting`
and `EmptySnapshot` render as `-32002`, and that cursor faults render as `-32602`
per the MCP pagination spec. That mapping is transport policy and moves to
`kenn-mcp`:

```rust
// kenn-mcp/src/server/errors.rs
pub const fn json_rpc_code(e: &QueryError) -> i32 { … }
```

The string codes (`EMPTY_SNAPSHOT`, `EMBEDDER_STARTING`) stay on the error, since
the CLI renders them too and `code_strings_stable` guards them.

## Sequencing

The signature change is what makes this a refactor rather than a move, so it goes
first, in place, while everything is still one crate and the compiler can
enumerate call sites:

```
1. QueryCtx, in place          21 with_db sites; queries stop seeing ServerState
2. McpError → QueryError       rename in place; json_rpc_code → server/errors.rs
3. move files to kenn-query    now a mechanical move; nothing left points back
4. register the 4 axis tools   the payoff — atlas-tables 3.5
```

An earlier draft opened with "move `peer` off `ServerState`". That step was dropped
once the inventory was taken: it assumed `ServerState` moves to `kenn-query` and
would drag its one rmcp-typed field along. `ServerState` does not move. After
step 1 its only users are `server/`, `indexing/`, and `tools/lifecycle.rs` — all of
which stay in `kenn-mcp` — so `peer` stays too, and no query ever sees it.

Steps 1–3 leave a working tree at every commit and are individually revertible.
Step 4 cannot compile halfway, which is why it is last and why it is nothing but
`git mv` plus imports by then.

## Risks

**A silent behavior change in the gate split.** Moving the empty-snapshot
classification into `QueryCtx::open` changes *where* it runs, and the ordering
against the lifecycle gate must not flip: today `INDEX_UNAVAILABLE` wins over
`EMPTY_SNAPSHOT`, because `ready_view_or_err()` runs first. The mutation check in
tasks §5.2 exists for exactly this — reverse the order and a test must go red.

**`with_db` is `pub(crate)`.** Once queries live in another crate it must become
`pub`, which widens `kenn-mcp`'s surface. Acceptable: the CLI already needs a way
to run the gate, and today it gets it implicitly by calling a tool.

**The CLI keeps depending on both crates.** It calls `kenn-query` for the queries
and `kenn-mcp` for `kenn server` and the lifecycle gate. That is not a defect of
the split — the CLI genuinely hosts both roles.

## Alternatives rejected

**Rename `kenn-mcp` to `kenn-query` and move `server/` out instead.** Same
boundary, mirrored. Rejected because `kenn-mcp` is the crate other things name in
`Cargo.toml` and in the MCP config the user's editor loads; keeping that name
attached to the MCP server is worth more than keeping it attached to the larger
half.

**Leave it and fix the doc comment.** Considered seriously — it costs ten lines
and the extraction buys nothing at runtime. Rejected because the mislabelling has
already produced a wrong architectural claim in review, and because the
`QueryCtx` refactor is worth doing on its own merits: it is what makes queries
testable without a server, and the crate move is then nearly free.
