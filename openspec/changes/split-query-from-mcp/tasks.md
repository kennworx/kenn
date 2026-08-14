## 1. Isolate the transport-only state

- [ ] 1.1 Move `peer: OnceLock<Peer<RoleServer>>` off `ServerState` into `indexing/`,
  its only reader — one write (`orchestrate.rs:26`) and three reads
  (`orchestrate.rs:637`, `roots.rs:216`, plus the `resolve_roots_and_maybe_rebind`
  parameter). → verify: `rg -n 'rmcp' crates/kenn-mcp/src/tools/` returns comment
  lines only, no `use` and no type.
- [ ] 1.2 Confirm nothing else in `tools/` outside `lifecycle.rs` reaches a
  daemon-only field. → verify: `state.lifecycle`, `state.watcher`, and
  `state.watcher_state` appear only in `tools/lifecycle.rs`.

## 2. `QueryCtx` — queries stop seeing `ServerState`

- [ ] 2.1 Add `QueryCtx` carrying the open reader, the snapshot id, `source_root`,
  `config`, the embedder stage/error pair, and `is_stale` — the six things `tools/`
  actually reads off `ServerState` today, and nothing else. → verify: it holds no
  `lifecycle`, `watcher`, `peer`, `layout`, or `model_id`.
- [ ] 2.2 Move the empty-snapshot classification into `QueryCtx::open`, leaving the
  lifecycle gate in `ServerState::with_db`. The two halves of today's `with_db` are
  different kinds of fact: `INDEX_UNAVAILABLE` describes a running daemon,
  `EMPTY_SNAPSHOT` describes a snapshot and a config that a query already holds. →
  verify: `QueryCtx::open` returns `EMPTY_SNAPSHOT` for an indexed-but-empty
  workspace with no lifecycle in scope.
- [ ] 2.2a Preserve the gate ORDER: `INDEX_UNAVAILABLE` continues to win over
  `EMPTY_SNAPSHOT`, because the lifecycle is checked before the context is built. →
  verify: a not-yet-`Ready` server with an empty snapshot returns
  `INDEX_UNAVAILABLE`, not `EMPTY_SNAPSHOT`.
- [ ] 2.2b Keep `with_db_allow_empty` working by mapping it to
  `QueryCtx::open_allow_empty`, so `get_workspace_overview` still succeeds on an
  empty snapshot and carries the hint in its response rather than erroring. →
  verify: `get_workspace_overview` on an empty workspace returns a result, not an
  error.
- [ ] 2.3 Change every query's first argument from `&ServerState` to `&QueryCtx`,
  across the 27 `with_db` sites. Let the compiler enumerate them — a signature
  change is exhaustive where a grep is not (CLAUDE.md §"let the compiler enumerate
  the call sites"). → verify: `cargo check --workspace` is the only thing consulted
  to find call sites; no grep-driven edit list.
- [ ] 2.4 Prove the point of the refactor by writing one query test that constructs
  no `ServerState` at all. → verify: a test calls `list_tables` against a reader and
  a default config, with no lifecycle driven to `Ready` and no server started.

## 3. `McpError` → `QueryError`

- [ ] 3.1 Rename the type and move `json_rpc_code()` to `kenn-mcp`'s wire layer,
  leaving the variants and their stable string codes on the error. The variants are
  query-domain facts; the JSON-RPC numbering is what MCP does with them. → verify:
  `code_strings_stable` passes unchanged, and no numeric JSON-RPC code appears
  outside `kenn-mcp`.
- [ ] 3.2 Confirm the CLI still renders the string codes it renders today. → verify:
  a CLI query against an empty workspace prints `EMPTY_SNAPSHOT` and its config
  hint, as before.

## 4. The crate move

- [ ] 4.1 Create `crates/kenn-query` and move `tools/` (minus `lifecycle.rs`),
  `types.rs`, `cursor.rs`, `result_cache.rs`, and the error module into it. By this
  point the move is mechanical: nothing in the moved set points back. → verify:
  `kenn-query/Cargo.toml` has no `rmcp` dependency.
- [ ] 4.2 Make `kenn-mcp` depend on `kenn-query` and keep `server/`, `indexing/`,
  `watcher.rs`, `state.rs`, and `tools/lifecycle.rs`. → verify: the 35 `#[tool]`
  wrappers compile against the moved functions with no change to their bodies beyond
  the path.
- [ ] 4.3 Repoint `kenn-cli`'s ~40 `tools::` call sites, keeping its `kenn-mcp`
  dependency for `kenn server` and the lifecycle gate. → verify: `kenn find`,
  `kenn get`, `kenn search`, and all five axis verbs answer identically to before.
- [ ] 4.4 Document the layering at the top of `kenn-query/src/lib.rs` — what the
  crate answers, its two front ends, and the rule that it may not depend on a
  transport. This is the artifact that stops the next reader repeating the
  misreading that prompted the change. → verify: the module doc names both
  consumers.

## 5. Verification — no behavior change

- [ ] 5.1 `just test` green with no test modified except for import paths and the
  new one in 2.4. A refactor that needed an assertion changed would not be one. →
  verify: the diff touches no `assert!` outside 2.4.
- [ ] 5.2 Mutation-check the gate split (§9): reverse the order so the context is
  built before the lifecycle is checked, and confirm 2.2a's test goes red for the
  stated reason — an empty snapshot on a not-yet-`Ready` server reporting
  `EMPTY_SNAPSHOT`. Restore and confirm green. → verify: the mutation fails that
  test and no other.
- [ ] 5.3 Mutation-check the dependency rule: add an `rmcp` import to `kenn-query`
  and confirm it does not compile. A layering rule with no mechanical enforcement is
  a comment. → verify: the build fails on the missing dependency, and the failure is
  the enforcement.
- [ ] 5.4 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all` and clippy once more (CLAUDE.md §7). Watch `with_db` and the
  `#[tool]` wrappers for the gate: splitting a function usually helps CRAP, but
  `QueryCtx::open` inherits branches from both halves of the old gate.

## 6. The payoff — `atlas-tables` 3.5

- [ ] 6.1 Register `list_packages`, `list_domains`, `list_contracts`, and
  `list_tables` as MCP tools. All four already return `ListResponse<T>` over
  `JsonSchema` args and are proven by the CLI; what blocked them was the ambiguity
  this change removes, not a design question about any one axis. → verify: the four
  appear in `tools/list` and each answers over MCP identically to its CLI verb.
- [ ] 6.2 Close `atlas-tables` 3.5, replacing its "the premise is wrong" note with a
  pointer here. → verify: `atlas-tables/tasks.md` records where the axis was
  exposed and why it waited.
