## 1. Establish the boundary by measurement

- [x] 1.1 ~~Move `peer` off `ServerState` into `indexing/`~~ — **dropped, the premise
  was wrong.** It assumed `ServerState` moves to `kenn-query`, which would drag its one
  rmcp-typed field along. It does not move: once queries take a `QueryCtx`, the only
  remaining users of `ServerState` are `server/`, `indexing/`, and `tools/lifecycle.rs`,
  all of which stay. `peer` stays with them and no query ever sees it. → verify: the
  inventory below shows zero query reads of `peer`.
- [x] 1.2 Confirm nothing in `tools/` outside `lifecycle.rs` reaches a daemon-only
  field. → verify: `lifecycle`, `watcher`, `watcher_state`, and `peer` appear in query
  files only in module docs and in `tests.rs` setup — **zero** production reads. The
  boundary is real, not aspirational.
- [x] 1.3 Inventory what queries DO reach for, since 2.1 was written from an estimate.
  Count by name, not by `state\.`: most calls wrap as `state\n    .with_db(`, and the
  receiver-anchored regex silently undercounts them (21 real `with_db` sites, not 1).

  | reached for | sites | in `QueryCtx`? |
  |---|---|---|
  | `read` / `snapshot_id` (via `ReadyView`) | 34 / 17 | yes |
  | findings store (`with_findings_read`/`_write`) | 24 | yes |
  | `with_db` | 21 | becomes `QueryCtx::open` |
  | `source_root()` | 6 | yes |
  | `search_symbols_cache` / `search_findings_cache` | 2 / 2 | yes |
  | `embed_stage` | 2 | yes |
  | `config` / `config_present()` | 2 | yes |
  | `layout` | 1 | yes |
  | `embed_error`, `is_stale` | 0 | no — the estimate was wrong |

  The embedder is NOT on `ServerState` — it is process-global, reached through
  `tools/support.rs`, so it needs no context field at all.

## 2. `QueryCtx` — queries stop seeing `ServerState`

- [x] 2.1 Add `QueryCtx` carrying exactly what §1.3 measured — the open reader, the
  snapshot id, `source_root`, `config`, `embed_stage`, both result caches, the findings
  store, and `layout` — and nothing else. → verify: it holds no `lifecycle`, `watcher`,
  `peer`, `watcher_state`, or `model_id`; `embed_error` and `is_stale` are absent
  because no query reads them.
- [x] 2.2 Move the empty-snapshot classification into `QueryCtx::open`, leaving the
  lifecycle gate in `ServerState::with_db`. The two halves of today's `with_db` are
  different kinds of fact: `INDEX_UNAVAILABLE` describes a running daemon,
  `EMPTY_SNAPSHOT` describes a snapshot and a config that a query already holds. →
  verify: `QueryCtx::open` returns `EMPTY_SNAPSHOT` for an indexed-but-empty
  workspace with no lifecycle in scope.
- [x] 2.2a Preserve the gate ORDER: `INDEX_UNAVAILABLE` continues to win over
  `EMPTY_SNAPSHOT`, because the lifecycle is checked before the context is built. →
  verify: a not-yet-`Ready` server with an empty snapshot returns
  `INDEX_UNAVAILABLE`, not `EMPTY_SNAPSHOT`.
- [x] 2.2b Keep `with_db_allow_empty` working by mapping it to
  `QueryCtx::open_allow_empty`, so `get_workspace_overview` still succeeds on an
  empty snapshot and carries the hint in its response rather than erroring. →
  verify: `get_workspace_overview` on an empty workspace returns a result, not an
  error.
- [x] 2.3 Change every query's first argument from `&ServerState` to `&QueryCtx`,
  across the 27 `with_db` sites. Let the compiler enumerate them — a signature
  change is exhaustive where a grep is not (CLAUDE.md §"let the compiler enumerate
  the call sites"). → verify: `cargo check --workspace` is the only thing consulted
  to find call sites; no grep-driven edit list.
- [x] 2.4 Prove the point of the refactor by writing one query test that constructs
  no `ServerState` at all. → verify: `kenn-query/tests/standalone.rs` builds a
  `QueryCtx` from literals over a bare reader — no lifecycle, no server, and the
  crate it lives in cannot even name `ServerState`.

  **Its first draft was a false guard**, caught by mutation (§9): querying a name
  the corpus did not contain still passed. Cause was the *fixture*, not the test —
  `find_symbol`'s last tier is n-gram fuzzy, so against a **one-symbol** corpus it
  returns that symbol for any input at all. Textbook §9 "suspect the fixture": the
  setup made the assertion true for a reason production would not reproduce. A
  second symbol makes the name discriminate; the same mutation now fails with 0
  items, and a `match_kind == "exact"` assertion pins which tier answered.
- [x] 2.5 **Unplanned, and the largest single piece of work so far.** Un-nesting the
  query bodies moved their branches onto the enclosing functions, which had been
  scoring as though they had none — `list_packages` 151.5, `list_contracts` 88.8,
  `list_domains` 66.3 against a threshold of 30. Extraction alone could not fix it:
  it relocated the branches into helpers measuring **0%** coverage, because *no
  in-process test anywhere called these three queries*. `cli_smoke.rs` walks all five
  axes but spawns the binary as a child process, so `llvm-cov` never saw them.

  Added `crates/kenn-mcp/tests/axis_queries.rs` — the repo's first aggregate-node +
  analysis-table fixture — covering the named-lookup paths that a bare listing never
  runs. → verify: gate PASSED, 61 suites green, and each test mutation-checked (§9).

  The fixture was the work, again. Three separate floors had to be cleared before the
  domain path was reachable at all, and the first two drafts failed with an *empty
  listing* rather than a wrong value — the signature of a fixture that never reaches
  the branch:

  | floor | value | first draft |
  |---|---|---|
  | `MIN_DOMAIN_SIZE` | 4 members | had 3 |
  | `MIN_PKG_MEMBERS` | 2 per spanned package | `pkg-b` had 1 |
  | `MIN_DOMAIN_LINKS` | 2 cross-package edges | had 1 |

  The two added members join by `Calls`, not `Implements`, so the contract axis still
  sees exactly two implementers in two packages.

## 3. `McpError` → `QueryError`

- [x] 3.1 Rename the type and move `json_rpc_code()` to `kenn-mcp`'s wire layer,
  leaving the variants and their stable string codes on the error. The variants are
  query-domain facts; the JSON-RPC numbering is what MCP does with them. → verify:
  `code_strings_stable` passes unchanged, and no numeric JSON-RPC code appears
  outside `kenn-mcp`.
- [x] 3.2 Confirm the CLI still renders the string codes it renders today. → verify:
  a CLI query against an empty workspace prints `EMPTY_SNAPSHOT` and its config
  hint, as before.

## 4. The crate move

- [x] 4.1 Create `crates/kenn-query` and move `tools/` (minus `lifecycle.rs`),
  `types.rs`, `cursor.rs`, `result_cache.rs`, and the error module into it. By this
  point the move is mechanical: nothing in the moved set points back. → verify:
  `kenn-query/Cargo.toml` has no `rmcp` dependency.
- [x] 4.2 Make `kenn-mcp` depend on `kenn-query` and keep `server/`, `indexing/`,
  `watcher.rs`, `state.rs`, and `tools/lifecycle.rs`. → verify: the 35 `#[tool]`
  wrappers compile against the moved functions with no change to their bodies beyond
  the path.
- [x] 4.3 Repoint `kenn-cli`'s ~40 `tools::` call sites, keeping its `kenn-mcp`
  dependency for `kenn server` and the lifecycle gate. → verify: `kenn find`,
  `kenn get`, `kenn search`, and all five axis verbs answer identically to before.
- [x] 4.4 Document the layering at the top of `kenn-query/src/lib.rs` — what the
  crate answers, its two front ends, and the rule that it may not depend on a
  transport. This is the artifact that stops the next reader repeating the
  misreading that prompted the change. → verify: the module doc names both
  consumers.

## 5. Verification — no behavior change

- [x] 5.1 `just test` green with no test modified except for import paths and the
  new one in 2.4. A refactor that needed an assertion changed would not be one. →
  verify: the diff touches no `assert!` outside 2.4.
- [x] 5.2 Mutation-check the gate split (§9) — **the mutation cannot be written,
  and that is the stronger result.** The task assumed the gate order was a
  convention two statements could express either way. It is not: the
  empty-snapshot check calls `ConfigHint::classify(config, symbol_count, …)`,
  which early-returns `None` unless `symbol_count == 0`, and that count is read
  through the connection the *lifecycle* gate produces. There is no reader before
  `Ready`, so `EMPTY_SNAPSHOT` cannot be reached first — the order is enforced by
  a data dependency, not by statement order, and no future edit can silently
  reverse it. → verify: `open_query` takes `view.read` from `ready_view_or_err()`
  before `classify` can be called; the ordering test in `tools/tests.rs` still
  asserts the observable contract.
- [x] 5.3 Mutation-check the dependency rule: add an `rmcp` import to `kenn-query`
  and confirm it does not compile. A layering rule with no mechanical enforcement is
  a comment. → verify: the build fails on the missing dependency, and the failure is
  the enforcement.
- [x] 5.4 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all` and clippy once more (CLAUDE.md §7). Watch `with_db` and the
  `#[tool]` wrappers for the gate: splitting a function usually helps CRAP, but
  `QueryCtx::open` inherits branches from both halves of the old gate.

## 6. The payoff — `atlas-tables` 3.5

- [x] 6.1 Register `list_packages`, `list_domains`, `list_contracts`, and
  `list_tables` as MCP tools. All four already return `ListResponse<T>` over
  `JsonSchema` args and are proven by the CLI; what blocked them was the ambiguity
  this change removes, not a design question about any one axis. → verify: the four
  appear in `tools/list` and each answers over MCP identically to its CLI verb.

  **It was five, not four.** `list_documents` is missing from the line above and
  from `atlas-tables`'s own note, but `openspec/specs/mcp-server/spec.md` requires
  it by name under "Atlas axis read tools" alongside domains and contracts. Both
  task lists were written from the code (`tools/mod.rs`'s exports) rather than from
  the spec, and both inherited the same omission. Registering four of five would
  have left one axis off the surface for a reason nobody could have stated.

  Each wrapper is the same three lines the other 35 use — the work was the
  descriptions, which are the only thing an agent sees before deciding to call.
  → verified two ways: `end_to_end.rs` requires all five in `tools/list` (it failed
  naming `list_packages` when run against a stale binary, so the assertion is
  load-bearing), and driving JSON-RPC over stdio against this repo's snapshot gives
  output **byte-identical** to `kenn tables|contracts|documents|domains|packages
  --json` for every axis.

  One gotcha worth recording: `cargo test -p kenn-mcp --test end_to_end` does NOT
  rebuild the `kenn` binary that test spawns, so it can assert against a stale
  server. `just test` builds the workspace and is unaffected; a single-package
  inner loop on this test needs `cargo build -p kenn` first.
- [x] 6.2 Close `atlas-tables` 3.5, replacing its "the premise is wrong" note with a
  pointer here. → verify: `atlas-tables/tasks.md` records where the axis was
  exposed and why it waited.

## 7. What the move actually cost, measured

The design predicted §4 would be "mechanical", and for the ~5000 lines of query
code it was — `crate::error::`, `crate::types::`, `crate::cursor::`, and
`crate::result_cache::` all kept working unchanged, because those modules moved
*with* the code that reads them. The whole back-reference set was:

| back-reference from the moved set | sites |
|---|---|
| `crate::state::EmbedStage` | 1 |
| `crate::state::LifecycleState` (in the ServerState half of `tools/state.rs`) | 2 |
| `crate::indexing::*` (same) | 2 |
| `crate::watcher::*` (same) | 1 |
| anything in `crate::server` | **0** |

Three things did NOT move and had to be teased apart, each for a stated reason:

- **`EmbedStage` → `kenn-query/types.rs`, `AtomicEmbedStage` stays.** The enum is
  a query-visible fact — `find_similar` reads it to tell "still building" from
  "genuinely missing". The atomic cell is daemon machinery.
- **`IndexStatus` / `IndexStatusProgress` → `kenn-mcp/index_status.rs`.** They
  had been sitting in `types.rs` with the wire shapes, but every field describes
  the *server*, and `types.rs` was the only thing dragging `WatcherState` toward
  the query crate. Moving them out took that pull with it.
- **`ServerState`'s two cache fields → `QueryCaches`.** The context's cache
  fields were private, so a host outside the crate could not use the struct
  literal. Bundling them removed two `ServerState` fields and the body of
  `clear_result_caches`, and gives the pair the one rule they share: cleared
  together on snapshot rotation.

Unplanned, and worth recording: **`cargo fmt` tripped `too_many_lines` after a
green clippy**, exactly as CLAUDE.md §7 warns. `kenn_query::` is four characters
longer than `tools::`, so re-wrapping pushed `findings_action` from ≤100 to 101
lines. Fixed by importing the argument *shapes* by name while leaving the query
*functions* path-qualified — which is what `server/core.rs` already did with the
same two sets, so the CLI now reads the same way.
