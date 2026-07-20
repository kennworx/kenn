# Design

## The command tree

```
kenn
├── overview                         get_workspace_overview   (singleton, like `status`)
├── find [<query>]                   semantic_search          ← bare query = default action
│    ├── symbol <name>               find_symbol
│    ├── symbols <query>             search_symbols
│    ├── at-location <file> <line>   find_at_location
│    ├── similar <id>                find_similar
│    └── usages <query>              find_usages
├── list <sub> <id>
│    │  callers            list_callers          overrides         list_overrides
│    │  callees            list_callees          usages            list_usages
│    │  implementers       list_implementers     correspondences   list_correspondences
│    │  in-scope           list_in_scope         imports           list_imports
│    │  module-files       list_module_files
├── check <links|css|findings>       check_links / check_css / check_anchors
├── findings <sub>
│    │  get <fnd_id>       get_finding           merge <ids…>      merge_findings   (write)
│    │  search <query>     search_findings       directives <p…>   find_directives
│    │  add <text>         store_finding (write) predecessors <id> find_predecessors
│    │  touch <fnd_id>     record_anchor (write) successors <id>   find_successors
└── get <symbol|source> <id>         get_symbol / get_source
```

29 tools placed. `status`→`get_index_status` and `index`→`reindex` already
exist; `wait_for_index` / `watch_*` / `debug_env` are out (see proposal).

## D1 — Verb-first grouping, and the predicate that makes it guessable

Five groups + one singleton instead of 29 flat top-level commands. The split is
not cosmetic; each verb encodes *what kind of operation it is*, so a user can
predict where a tool lives:

- **`find`** — *search / resolve*: a query or name in, ranked or fuzzy
  candidates out.
- **`list`** — *enumerate edges*: an already-known id in, exact graph rows out.
- **`check`** — *diagnostic sweep*: no target, problems out.
- **`findings`** — the knowledge store. A **noun** group on purpose: it is a
  distinct subsystem (store + DAG + directives + anchors), not a code-graph
  read, so it gets its own namespace rather than being scattered across the
  verbs.
- **`get`** — *fetch one entity by exact id* → full detail.

## D2 — Output: TOON default, `--json` opt-out

The tabular win is **shape-specific**, not universal. Of the 29 mirrored tools,
~15 return `ListResponse<T> { items, next }` — a uniform array, where JSON
repeats every field name on every row and TOON declares the header once:

```
items[2]{id,kind,language,name,location,package,module,nargs,targs,external,test,partial}:
  cs:Orders.Handler,method,csharp,Handle,./src/Orders/Handler.cs#42-88,Orders,cs:Orders,2,0,false,false,false
  cs:Orders.Service,method,csharp,Process,./src/Orders/Service.cs#10-30,Orders,cs:Orders,1,0,false,false,false
next: null
```

On a 25–50 row page that is ~40–60% fewer tokens and more skimmable — this is
the `find` / `list` surface, and the main reason TOON is the default.

The other ~14 are **not** flat tables and TOON gives no header-once win on them,
though it still renders (as nested key:value, more compact than JSON):
`SingleResponse<T>` (`overview`, `get symbol|source`, `findings get`), the
diagnostic shapes (`check links|css|findings`), `find usages`
(`FindUsagesResponse`), the write echoes (`findings add|merge|touch`), and —
worth calling out — **`find <query>` / `semantic_search`, which returns two
independently-ranked groups (code + findings), not one table.** The renderer
must handle nested objects and multiple arrays, not assume a single `items`.

`--json` yields the same JSON value the MCP server returns (pretty-printed;
byte-for-byte differs from MCP's compact `Content::json`, the value does not).

**Decision — depend on the `toon` crate** (`toon = "0.1"`), wrapped so the one
call site is swappable. Integration is `to_value(resp)? → toon::encode(&v, opts)`
(our types are all `Serialize`; the crate is `serde_json::Value`-based, MIT, and
deps `serde_json` + `regex` — both already in our lockfile, `regex` via
`tracing-subscriber`'s `env-filter` — so net-new transitive cost is ~zero).

Why not vendor: TOON's hard part is **quoting/escaping**, and it is on our hot
path — `findings get|add`, `get source`, and `get symbol` return free-form text
(finding bodies, source spans, doc comments) that routinely contains commas,
colons, newlines, leading `-`, and number/bool/null look-alikes. Per the TOON
spec a correct encoder must quote on ~8 conditions and apply the `\n \r \t \" \\
\uXXXX` escape table, plus tabular-qualification detection with list-form
fallback. That is ~500–600 lines of correctness-critical logic (the crate is
608) — a naive shorter vendor silently corrupts exactly those free-form
commands. Reimplementing it would also carry a real CRAP (§6) + `clippy::pedantic`
(§5) tax on branchy string code, for a prototype with no external users.

The crate's risk (0.1.2, one maintainer, untouched since Oct 2025) is cheap to
insure: it is a pure `Value → String` function behind a stable written spec,
MIT-licensed so we can **fork-to-vendor** its source the day it rots, and its
blast radius is a single CLI renderer (MCP is untouched). Explicitly rejected:
copying the 600 lines into our tree now — that inherits the CRAP/pedantic tax
*and* loses upstream fixes. Depend now; fork only on actual breakage.

## D3 — Bare `find <query>` as the default action, and its sharp edge

`kenn find <query>` with no subcommand runs `semantic_search` (dropping an
explicit `semantic` subcommand). clap resolves the first token as a subcommand
*before* treating it as a positional, so a **single-word query equal to a
subcommand name is swallowed**:

```
kenn find similar            → the `similar` subcommand (wants <id>, errors)
kenn find order flow         → semantic search ✓  (multi-token, unambiguous)
kenn find "auth"             → semantic search ✓  (not a subcommand word)
```

Multi-word semantic queries — the common case — are safe. Mitigation is a line
in `--help`, not a redesign; `kenn find -- similar` forces query interpretation
if ever needed. Accept the edge.

## D4 — Hide "anchor" from the surface

The word "anchor" is internal. On the CLI:

```
  a finding is pinned to files ─┐
  check findings   → sweep for pins whose file moved/vanished   (check_anchors)
  findings touch   → re-confirm / move / drop a pin             (record_anchor)
                     default --op attach = "still applies here"; --op detach|rename
```

`touch` mirrors `touch(1)`: default is re-confirm-liveness (`attach`).

## D5 — Keep `get`, shrunk to two

`get_workspace_overview` becomes the `overview` singleton and `get_finding`
moves under `findings`, so `get` is left with only the by-exact-id fetches
`get_symbol` / `get_source`. These are **not** folded into `find`: `find` means
*search*, and a `find symbol` that sometimes searches a name and sometimes
fetches an id depending on argument shape is exactly the magic that misfires.
A 2-item `get` is the honest home.

## D6 — Shared flags

The tool `Filters` splits into two tiers:

- **`--include-tests` / `--include-external`** are **global** (defined once on
  `Cli`, `global = true`, like `--workspace`), so they read uniformly on every
  command. Each is an optional-value bool: bare `--include-tests` = `true`,
  explicit `--include-tests=true|false`, absent = the **universal default of
  `false`**. The CLI sends the resolved value **explicitly** (always `Some`) on
  every tool that accepts it. This gives one predictable default across the
  surface and, unlike a bare flag, can express an explicit `false` (needed to
  narrow a `list callers` to non-test callers).

  The **MCP tools were changed to match**: the graph-walk tools
  (`list_callers`/`callees`/`implementers`/`overrides`/`usages`/`in_scope`/
  `imports`/`correspondences`) previously defaulted `include_tests=true`
  ("refactor scope includes test callers"), and `find_usages` hard-coded
  `include_tests=true`. All now default `include_tests=false` (overridable),
  so CLI and MCP share one universal default and `find_usages` gained a real
  `include_tests` param. This is an agent-facing behavior change: a bare
  `list_callers` no longer includes test callers — pass `include_tests: true`
  for the full refactor surface. `include_external` already defaulted `false`
  everywhere, so it was unchanged.
- **`--kind` / `--language` / `--package` / `--file`** stay per-command (a
  `FilterArgs` block flattened only into commands whose tool takes `filters`).

```
(global)  --include-tests[=BOOL]   --include-external[=BOOL]
(per-cmd) --kind <k>…   --language <l>…   --package <p>…   --file <f>…
          --page-size <n>   --cursor <tok>   --all
(global)  --json    --workspace <p>   --config <p>
```

`--all` drains the `next` cursor loop (nobody hand-threads opaque tokens);
`next:` still appears in output so scripts can page manually, and the last
page's non-`items`/`next` fields (e.g. `find_usages` `truncated`) are
preserved. `--json` is valid on every query command.

`list imports` is the one tool whose `direction` is **required** upstream
(`ListImportsArgs.direction` is not `Option`). The CLI SHALL default
`--direction both` so `kenn list imports <id>` works with no flag, showing
inbound + outbound (each row tagged with its direction).

The write commands carry structured args beyond their positional:
`findings add <text>` takes repeatable `--parent` / `--tag` / `--anchor`;
`findings merge <ids…>` takes a **required** `--text` (plus `--tag`);
`findings touch <fnd_id>` takes `--anchor` (for `attach`/`detach`) or
`--from` / `--to` (for `rename`).

## D7 — Why this is glue

- `pub mod tools;` in `kenn-mcp/src/lib.rs` → `kenn_mcp::tools::find_symbol`
  and all arg structs + `ServerState` are already reachable from `kenn-cli`
  (already a dependency).
- `cmd_mcp` already constructs `ServerState::with_layout_config_and_model` and a
  multi-thread tokio runtime. Each query command reuses that pattern:
  `parse argv → build state → rt.block_on(tools::X(&state, &args)) → emit`.
- The MCP server's own wrapper is `json_result(tools::X(...).await)`. The CLI
  differs only in the sink: `emit(value, format)` instead of `Content::json`.

## D8 — Wrinkles

- **Vector cold-start.** Pure graph reads (`list *`, `get *`, `find symbol`,
  `find at-location`, `find usages`) are instant. The **query-embedding** set —
  established at implementation time by grepping `embed_query` call sites, not
  assumed — is: bare `find` (`semantic_search`), **`find symbols`**
  (`search_symbols` blends lexical + vector), **`findings search`**, **`findings
  add`** and **`findings merge`** (embed the finding text), and `findings
  directives --query`. Notably **`find similar` does NOT embed** at query time —
  it reuses the symbol's committed vector. A one-shot process has no daemon to
  reuse, so `run_on_state` pre-warms the embedder (`embed_block_until_ready`)
  for exactly this set before dispatch, so the first (and only) call doesn't
  surface `EMBEDDER_STARTING`. The `embeds` flag is computed per subcommand in
  each entry fn.
- **Freshness.** A one-shot CLI reads the live snapshot with no watcher /
  auto-reindex. That is correct and expected; `kenn status` remains the
  staleness signal.
