# mcp-server

## Purpose

The `kenn mcp` server exposes a read-only code-graph API over MCP
stdio. Its contract: bind stdio immediately, gate non-status tools on a
populated index, emit progress notifications, and dispatch tool calls
async end-to-end against the snapshot DB.
## Requirements
### Requirement: Server binds stdio without a pre-existing snapshot

The MCP server SHALL bind its stdio transport and accept tool calls
immediately on launch, regardless of whether `.kenn/live/` exists.
Indexing, if needed, runs in the background; it does not block the
transport bind.

#### Scenario: Launch in unindexed workspace

- **WHEN** `kenn mcp <ws>` is launched in a workspace with no
  `.kenn/live/`
- **THEN** the MCP server completes its rmcp `initialize` handshake
  within typical handshake latency (no blocking on indexing)
- **AND** subsequent `tools/list` calls return the full tool list
- **AND** `tools/call get_index_status` returns the current lifecycle
  state

### Requirement: Tools other than `get_index_status` fail fast while not Ready

The MCP server SHALL return a JSON-RPC error with code
`INDEX_UNAVAILABLE` for every tool call while its lifecycle state is
`Indexing` or `Failed`, **except for the status-class tools
`get_index_status` and `wait_for_index`**, which SHALL succeed in every
state. The error message MUST include the current state (e.g. "indexing
in progress" or "indexing failed: <reason>") so the agent can decide
whether to retry or surface to its operator.

The `get_index_status` tool SHALL succeed in every state and SHALL
return a structured payload describing the lifecycle state.
`wait_for_index` likewise SHALL succeed in every state — its purpose is
to be called precisely while the server is not yet Ready.

(The requirement name is retained for continuity; the normative set of
exempt tools is `get_index_status` and `wait_for_index`.)

#### Scenario: search_symbols during indexing

- **GIVEN** the MCP server is in the `Indexing` state
- **WHEN** the agent calls `tools/call search_symbols { ... }`
- **THEN** the response is a JSON-RPC error with code
  `INDEX_UNAVAILABLE`
- **AND** the error message contains the string `"indexing"`

#### Scenario: get_index_status during indexing

- **GIVEN** the MCP server is in the `Indexing` state with batch
  progress recorded
- **WHEN** the agent calls `tools/call get_index_status`
- **THEN** the response is success with `state: "indexing"`
- **AND** the response includes `progress` fields (files seen,
  symbols seen, current phase)
- **AND** the response is returned without delay (< 100ms)

#### Scenario: wait_for_index permitted during indexing

- **GIVEN** the MCP server is in the `Indexing` state
- **WHEN** the agent calls `tools/call wait_for_index { ... }`
- **THEN** the response is NOT an `INDEX_UNAVAILABLE` error
- **AND** the call blocks (up to its timeout) rather than failing fast

#### Scenario: Tools become available after Ready

- **GIVEN** the MCP server has transitioned `Indexing → Ready`
- **WHEN** the agent calls any tool, e.g. `get_workspace_overview`
- **THEN** the tool returns its normal success payload, not
  `INDEX_UNAVAILABLE`

### Requirement: get_index_status returns lifecycle state

The `get_index_status` tool's response payload SHALL include a `state`
string field with one of `"indexing"`, `"embedding"`, `"ready"`, `"disabled"`,
or `"failed"`. These form the pipeline progression `indexing → embedding → ready`,
where `embedding` is the window in which the code graph is built but the background
embedding pass is still filling vectors, `disabled` replaces the `embedding → ready`
arc when no embedder is configured (vectors will not be built), and `failed` is a
cold-start index failure.

**Structural-vs-vector contract.** From the `embedding` stage onward the code graph
is queryable: structural tools (`find_symbol`, `find_usages`, `list_callers`, etc.)
SHALL succeed during `embedding`, `ready`, and `disabled`. Only vector tools
(`find_similar`, `semantic_search`) depend on the embedding pass. An agent that needs
only structural queries SHALL NOT wait for `ready` — `embedding` is sufficient. The
`embedding` and `disabled` states therefore behave like `ready` for the
not-Ready fast-fail gate (structural tools serve; they are not blocked).

When `state` is `"indexing"`, the payload SHALL include a `progress`
object with at least:
- `phase` (string) — current pipeline phase identifier
- `files_seen` (number)
- `symbols_seen` (number)

When `state` is `"failed"`, the payload SHALL include an `error`
string describing the failure.

When `state` is `"embedding"`, `"ready"`, or `"disabled"`, the existing fields
(`snapshot_id`, `indexed_at`, `is_stale`, `reindex_in_progress`,
`fallback_from_parent_worktree`) SHALL all be populated as in the prior `"ready"`
payload.

#### Scenario: Status during indexing carries progress

- **GIVEN** the server is in `Indexing` and has processed two batches
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "indexing"`
- **AND** `progress.phase` is a non-empty string
- **AND** `progress.files_seen` and `progress.symbols_seen` are
  non-negative numbers

#### Scenario: Status after failure carries error

- **GIVEN** the server is in `Failed` because the indexer subprocess
  exited with a non-zero status
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "failed"`
- **AND** `error` is a non-empty string describing the failure

#### Scenario: Status reports embedding while the background pass runs

- **GIVEN** the code graph is built and the background embed pass is running
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "embedding"`
- **AND** structural tools (e.g. `find_symbol`) succeed rather than fail-fast

#### Scenario: Status reports disabled when no embedder is configured

- **GIVEN** the code graph is built and no embedder is configured
- **WHEN** the embed pass completes
- **THEN** `get_index_status` reports `state: "disabled"`
- **AND** structural tools still succeed

### Requirement: Progress notifications during indexing

While indexing is in progress, the MCP server SHALL emit rmcp
`notifications/message` log entries at info level summarizing pipeline
progress. Notifications SHALL be emitted at least:

- Once at the start of indexing.
- Once when the data ingest phase completes.
- Once at significant milestones (per implementation choice — typically
  per batch flush).
- Once when indexing finishes (success or failure).

Agents SHALL be able to observe indexing progress without polling
`get_index_status`, by listening for these notifications.

#### Scenario: Indexing emits start and end notifications

- **WHEN** the MCP server starts and triggers indexing
- **THEN** an info-level `notifications/message` is emitted with a
  payload signaling indexing started
- **AND** when indexing completes, a final info-level notification is
  emitted signaling completion

### Requirement: Tool dispatch is async end-to-end

The MCP server SHALL dispatch tool calls through async functions all the
way down to the storage layer. Tools MUST NOT route through
`tokio::task::spawn_blocking` for the sole purpose of preventing a nested
tokio runtime.

The storage **read** path SHALL execute its blocking SQLite work (connection
use and queries) on a dedicated per-snapshot connection pool, not on a
runtime worker thread. Concretely, the `Ready` snapshot binding SHALL hold a
read-only connection pool (opened once when the snapshot is bound) and tool
reads SHALL run their queries through it, so that (a) blocking SQLite never
occupies a runtime worker for the duration of the I/O, and (b) concurrent
reads proceed on separate connections rather than serializing behind a single
shared connection. The pool MUST NOT open a fresh connection per tool call on
the hot path.

The wire-level tool contract (input/output shapes, JSON-RPC error codes
including `INDEX_UNAVAILABLE` and `EMPTY_SNAPSHOT`, pagination, and progress
notifications) is unchanged.

#### Scenario: Blocking storage work does not occupy a runtime worker

- **GIVEN** the MCP server is in `Ready` state
- **WHEN** the agent issues a `tools/call` for any read tool
- **THEN** the tool's SQLite open/query runs on the snapshot pool's
  connection threads, not on the rmcp runtime's worker threads
- **AND** no `spawn_blocking` is involved in the storage path

#### Scenario: Concurrent reads do not serialize

- **GIVEN** the MCP server is in `Ready` state with a multi-connection pool
- **WHEN** several read `tools/call`s are in flight at once
- **THEN** they execute on separate pool connections in parallel
- **AND** one slow read does not block the others behind a single connection

#### Scenario: Wire contract is preserved

- **WHEN** an agent calls `get_workspace_overview` against a Ready server
- **THEN** the response payload is byte-for-byte equivalent to the
  pre-pool version (same fields, same shapes)
- **AND** the same call against an `Indexing` server still returns
  `INDEX_UNAVAILABLE` with the same code and message form

### Requirement: get_symbol tolerates non-unique pub_id

`get_symbol(pub_id)` SHALL NOT assume `pub_id` is unique across the
`symbols` table. The DB schema permits multiple rows with the same
`pub_id` when they belong to different packages.

Until the in-flight MCP-server redesign replaces the surface, the
internal implementation SHALL return the first matching row (ordered
by `short_id ASC`) and MUST NOT panic, error, or otherwise fail when
multiple rows match. The returned `SymbolRef` envelope SHALL include
the resolving package's `name` and `version` so the agent can detect
the multi-match case and follow up.

The concrete tool-level behavior for the multi-match case (return all,
require `pkg` to disambiguate, prefer one over another) is owned by
the MCP-server redesign and is not pinned by this proposal. This
proposal commits only to (a) the data-model invariant that
`(pub_id, pkg)` uniquely identifies a symbol, and (b) that the
existing tool surface keeps working under the relaxed uniqueness.

#### Scenario: Multi-version package does not crash get_symbol

- **WHEN** the workspace transitively depends on two versions of the
  same package, each declaring a symbol with the same `pub_id`
- **AND** the agent calls `get_symbol(id)` with that `pub_id`
- **THEN** the call MUST succeed
- **AND** MUST return one of the matching rows
- **AND** the response envelope MUST identify the resolving package

### Requirement: Locations rendered as path#startLine-endLine

Locations returned by MCP tools SHALL be rendered in the form
`<file_path>#<start_line>-<end_line>` using line numbers from the
`defs` table. Column data is not included in the default rendering.

When an agent needs precise column ranges (e.g., for highlighting a
specific identifier within a line), tool implementations MAY include a
secondary structured field carrying the four-tuple
`(start_line, start_col, end_line, end_col)` from `defs`. This is an
optional extension; the default surface stays line-only.

For partial symbols, the response SHALL include all declaration sites
from `defs` (one rendered location per site).

#### Scenario: Default rendering uses line range only

- **WHEN** an MCP tool returns a symbol's location
- **THEN** the rendered form MUST match
  `<path>#<start_line>-<end_line>`
- **AND** column numbers MUST NOT appear in the rendered string

#### Scenario: Partial symbol returns multiple locations

- **WHEN** the agent calls `get_symbol(id)` for a symbol with
  `partial = true` and three `defs` rows
- **THEN** the response MUST include three rendered locations,
  one per declaration site

### Requirement: An unresolved entity reference is an error, not an empty result

A tool that takes an entity reference — a symbol `pub_id`, a file, or a finding id — SHALL return an `INVALID_INPUT` JSON-RPC error when that reference does not resolve in the live snapshot, rather than a success payload with an empty result set. An empty `items` array is reserved for a reference that *resolves* but genuinely has no matches (for example, a real symbol with no callers).

Tools that return an explicit `{found: false}` payload — `get_symbol`, `get_source`, `get_finding` — satisfy this requirement as-is: `{found: false}` is unambiguous. Search tools — `search_symbols`, `find_symbol`, `semantic_search`, `search_findings` — are exempt: an empty result is the correct answer to a query that matched nothing.

`find_at_location` SHALL address its file by `file_path`, a workspace-relative or absolute path; a path absent from the snapshot SHALL be an `INVALID_INPUT` error. No numeric file id SHALL appear on the tool surface — a per-run `short_id` carries no snapshot-stable meaning and would be a silent staleness hazard.

#### Scenario: navigating from a non-existent symbol id

- **WHEN** the agent calls `list_callers` with an `id` that resolves to no symbol
- **THEN** the response is an `INVALID_INPUT` error naming the id
- **AND** it is not an empty success payload

#### Scenario: a resolved symbol with no matches returns empty

- **WHEN** the agent calls `list_callers` for a symbol that exists but nothing calls
- **THEN** the response is a success payload with an empty `items` array

#### Scenario: find_at_location on an unindexed file

- **WHEN** the agent calls `find_at_location` with a `file_path` not present in the snapshot
- **THEN** the response is an `INVALID_INPUT` error

### Requirement: Paginated tool results MUST use opaque, server-controlled cursors

Every kenn-mcp tool that returns a list SHALL paginate its results using opaque cursor tokens that follow the MCP pagination contract. The paginated tools include `search_symbols`, `list_callers`, `list_callees`, `list_usages`, `list_in_scope`, `list_implementers`, `list_overrides`, `list_correspondences`, `list_imports`, `list_module_files`, `find_symbol`, and `find_similar`.

The cursor SHALL be opaque to callers. Callers MUST NOT parse, modify,
or persist cursors across sessions. The server SHALL decide page size;
the `limit` parameter on paginated tools is a server-controlled
ceiling capped at 200 server-side and MUST NOT be interpreted by
clients as a guaranteed page size.

The server SHALL emit a continuation cursor in the response if and
only if the underlying result stream has more rows after the returned
page. A final page MUST omit the cursor entirely. Clients receiving a
response without a continuation cursor MUST treat the stream as
exhausted.

#### Scenario: cursor opacity

- **WHEN** an agent calls a paginated tool and receives a continuation cursor
- **THEN** the agent treats the cursor as an opaque string with no documented format
- **AND** the only valid action is to pass it verbatim back to the same tool
- **AND** the agent does not parse, decode, modify, or persist the cursor across sessions

#### Scenario: nextCursor only when more

- **GIVEN** a paginated tool whose result set has exactly N rows
- **WHEN** the tool is walked with page size that exactly consumes all N rows in the final page
- **THEN** the final page response MUST omit any continuation cursor
- **AND** passing back a previously-issued cursor from this stream returns an empty page with no continuation cursor

#### Scenario: server-decided page size

- **GIVEN** a paginated tool with `limit` not specified by the caller
- **WHEN** the tool runs
- **THEN** the server returns a page of size determined by server policy (default 25, hard cap 200)
- **AND** the caller cannot assume any specific size before reading the response

### Requirement: Invalid and stale cursors MUST return `-32602`

The server SHALL return JSON-RPC `-32602 Invalid params` for any cursor that cannot be decoded (bad base64, wrong length, structural mismatch), per the MCP pagination spec.

A cursor that decodes correctly but references a snapshot that no
longer matches the live index (snapshot rotated between calls) SHALL
also produce `-32602 Invalid params`, with a kenn-specific subcode in
the error's `data` payload so the agent can distinguish "your cursor
was malformed" from "you need to restart pagination because the index
rotated."

The error's `data` payload SHALL include a `kenn_subcode` field with
one of:

- `"INVALID_CURSOR"` — the cursor could not be decoded.
- `"STALE_CURSOR"` — the cursor decoded but the snapshot no longer matches.

#### Scenario: malformed cursor

- **WHEN** an agent calls a paginated tool with a cursor whose length has been changed (truncated, padded, or whose base64 decodes to a wrong byte count for either the 10-byte list shape or 14-byte search shape)
- **THEN** the server returns `-32602 Invalid params`
- **AND** the error's `data.kenn_subcode` is `"INVALID_CURSOR"`

Note: a cursor whose content has been mutated *without* changing its
length usually decodes successfully but points to a non-existent or
wrong-snapshot position; that path returns either `STALE_CURSOR`
(when the snapshot prefix no longer matches) or a valid-shaped empty
page (when the position is past the data). The deterministic way to
trigger `INVALID_CURSOR` is a length-mutation.

#### Scenario: stale cursor across a snapshot rotation

- **GIVEN** an agent has a valid continuation cursor from a previous page
- **WHEN** the index rotates to a new snapshot between calls
- **AND** the agent passes the old cursor back
- **THEN** the server returns `-32602 Invalid params`
- **AND** the error's `data.kenn_subcode` is `"STALE_CURSOR"`
- **AND** the agent's correct action is to restart pagination from the beginning, not to "fix" the cursor

### Requirement: `tools/list` MUST conform to MCP pagination

The kenn-mcp `tools/list` response SHALL conform to the MCP
pagination contract: opaque cursor, server-decided page size,
`nextCursor` present only when more tools follow.

Kenn-mcp's tool count is small enough that `tools/list` typically
returns a single page; the contract holds for future growth and for
host conformance testing.

#### Scenario: tools/list single page

- **WHEN** a client calls `tools/list` and the full tool set fits in one server-decided page
- **THEN** the response is `{ tools: [...] }` with no `nextCursor` field
- **AND** the client treats this as the complete tool list

#### Scenario: tools/list cursor round-trip

- **GIVEN** a future kenn-mcp build large enough that `tools/list` paginates
- **WHEN** the client walks pages until `nextCursor` is absent
- **THEN** the union of returned tools equals the full tool set
- **AND** no tool appears in more than one page

### Requirement: Workspace resolution follows a five-step priority chain

The `kenn mcp` server SHALL resolve its bound workspace by consulting
the following sources in priority order, stopping at the first that
yields a usable local filesystem path:

1. The global `--workspace <path>` CLI flag, when provided.
2. The `CLAUDE_PROJECT_DIR` environment variable, when set to an existing local directory. Claude Code sets this on every MCP subprocess at spawn time (confirmed via the `debug_env` MCP tool against Claude Code 2.1.148).
3. The first `file://` root returned by `roots/list`, issued to the client after the rmcp `initialize` handshake, when the client declares the `roots` capability.
4. `git rev-parse --show-toplevel` from the launching process's cwd.
5. The launching process's cwd.

The flag (1) is retained for explicit operator invocations and for
the test harness; production MCP launches typically do not set it.
Source (2) is the Claude-Code-specific path — available
pre-handshake, no rmcp roundtrip. Source (3) is the protocol-clean
answer for any host that declares `roots`. Sources (4) and (5) are
legacy fallbacks preserved for manual `kenn mcp` invocations and for
backward compatibility.

Because source (3) requires the rmcp `Peer` and therefore cannot
resolve before the `initialize` handshake, the server SHALL bind
**tentatively** at startup using the highest-priority source
available pre-handshake (1, then 2, then 4, then 5) and SHALL rebind
to the result of source (3) after `on_initialized` if it differs.

#### Scenario: --workspace flag provided

- **WHEN** the server is launched as `kenn --workspace /home/user/proj mcp`
- **THEN** the server binds tentatively to `/home/user/proj` before the handshake
- **AND** the server does NOT consult `CLAUDE_PROJECT_DIR`, does NOT issue `roots/list` after the handshake, and does NOT rebind on `listChanged` — regardless of host capability
- **AND** the startup log records `source=cli-flag path=/home/user/proj`

#### Scenario: No flag, CLAUDE_PROJECT_DIR set, host supports roots, both agree

- **WHEN** the server is launched as `kenn mcp` (no flag) with `CLAUDE_PROJECT_DIR=/home/user/proj` in env
- **THEN** the server binds tentatively to `/home/user/proj` via the env var, before the handshake
- **AND** after the handshake the server issues `roots/list`, finds the bound workspace already matches `roots/list[0]`, and takes no action
- **AND** the startup log records `source=claude-project-dir path=/home/user/proj`

#### Scenario: No flag, CLAUDE_PROJECT_DIR set, host's roots disagrees

- **WHEN** the server is launched as `kenn mcp` (no flag) with `CLAUDE_PROJECT_DIR=/home/user/proj`
- **AND** after the handshake `roots/list` returns `[file:///home/user/other]`
- **THEN** the server rebinds to `/home/user/other` via the post-handshake path
- **AND** the startup log records both the tentative bind (`source=claude-project-dir path=/home/user/proj`) and the final rebind (`source=roots-list path=/home/user/other`)

#### Scenario: CLAUDE_PROJECT_DIR points at a non-existent path

- **WHEN** `CLAUDE_PROJECT_DIR=/does/not/exist` and no `--workspace` flag was given
- **THEN** the server rejects the env value, logs the rejection, and falls through to source (3) and below as if `CLAUDE_PROJECT_DIR` were unset

#### Scenario: No flag, no CLAUDE_PROJECT_DIR, host supports roots, workspaces match

- **WHEN** the server is launched as `kenn mcp` (no flag, no env) from inside `/home/user/proj` (a git repo)
- **AND** the host declares the `roots` capability
- **AND** `roots/list` returns `[file:///home/user/proj]`
- **THEN** the server binds tentatively to `/home/user/proj` via git-toplevel before the handshake
- **AND** after the handshake the server issues `roots/list`, finds the bound workspace already matches, and takes no action
- **AND** the startup log records `source=roots-list path=/home/user/proj` (the tentative source is superseded)

#### Scenario: No flag, no CLAUDE_PROJECT_DIR, host supports roots, workspaces differ

- **WHEN** the server is launched as `kenn mcp` (no flag, no env) from cwd `/` (no git repo)
- **THEN** the server binds tentatively to `/` via cwd before the handshake
- **AND** indexing is attempted against `/` and either fails fast or runs against minimal content
- **WHEN** the host declares `roots` and `roots/list` returns `[file:///home/user/proj]`
- **THEN** the server aborts the tentative indexing
- **AND** rebinds to `/home/user/proj`
- **AND** the startup log records `source=roots-list path=/home/user/proj`

#### Scenario: No flag, no env, no roots capability, git toplevel succeeds

- **WHEN** the server is launched as `kenn mcp` from inside `/home/user/proj` (a git repo)
- **AND** `CLAUDE_PROJECT_DIR` is unset
- **AND** the host does not declare the `roots` capability
- **THEN** the server binds to `/home/user/proj` via git-toplevel
- **AND** the startup log records `source=git-toplevel path=/home/user/proj reason=client-no-roots-capability`

#### Scenario: No flag, no env, no roots capability, no git

- **WHEN** the server is launched as `kenn mcp` from cwd `/tmp/x` (not in a git repo)
- **AND** `CLAUDE_PROJECT_DIR` is unset
- **AND** the host does not declare the `roots` capability
- **THEN** the server binds to `/tmp/x` via cwd
- **AND** the startup log records `source=cwd path=/tmp/x reason=client-no-roots-capability`

### Requirement: Multiple roots collapse to the first; non-`file://` roots are rejected

When `roots/list` returns more than one root, the server SHALL use
the first `file://` root and SHALL log the remaining roots as
ignored. Non-`file://` URIs SHALL be skipped and logged with their
scheme; the next eligible root in order becomes the active one. If
no `file://` root is found, the server SHALL retain whichever
tentative binding the pre-handshake chain produced — `claude-project-dir`,
`git-toplevel`, or `cwd` — and log that source with
`reason=client-roots-non-file` or `reason=client-roots-empty` as
appropriate. (The `reason` field is omitted when the tentative source
is `claude-project-dir`, since the chain succeeded at step 2 and
fallback semantics don't apply.)

Multi-root indexing (unioning multiple roots into a single index) is
out of scope for this requirement.

#### Scenario: Multiple file:// roots

- **WHEN** `roots/list` returns `[file:///a, file:///b, file:///c]`
- **THEN** the server binds to `/a`
- **AND** the startup log records `ignored_roots=[file:///b, file:///c]`

#### Scenario: Non-file scheme

- **WHEN** `roots/list` returns `[vscode-vfs:///x, file:///proj]`
- **THEN** the server binds to `/proj`
- **AND** the startup log records the skipped `vscode-vfs:///x` with reason `unsupported_scheme`

#### Scenario: Only non-file roots, no env var

- **WHEN** `roots/list` returns only non-`file://` URIs
- **AND** no `--workspace` flag was provided
- **AND** `CLAUDE_PROJECT_DIR` is unset
- **THEN** the server retains its tentative pre-handshake binding (`git-toplevel` or `cwd`)
- **AND** the startup log records the original source with `reason=client-roots-non-file`

#### Scenario: Only non-file roots, with env var

- **WHEN** `roots/list` returns only non-`file://` URIs
- **AND** `CLAUDE_PROJECT_DIR` was set and resolved to `/home/user/proj`
- **THEN** the server retains the `claude-project-dir` tentative binding
- **AND** the startup log records `source=claude-project-dir path=/home/user/proj` with no `reason` field (the chain succeeded at step 2)

### Requirement: `notifications/roots/list_changed` triggers a workspace rebind when supported

When the client declares `roots.listChanged: true`, the server SHALL
register a handler for `notifications/roots/list_changed`. On receipt
of the notification, the server SHALL re-issue `roots/list` and:

- If the resolved first root URI is unchanged, take no action.
- If it changed, the server SHALL rebind to the new workspace:
  close the current snapshot DB handle, point the bound workspace at
  the new path, and trigger indexing of the new workspace through
  the existing background indexing path.

In-flight tool calls against the old workspace MAY complete; new
tool calls SHALL be served from the new workspace. If indexing is
currently running on the old workspace, the server SHALL abort it
cleanly before binding the new workspace.

The transport MUST NOT block during the rebind; tools other than
`get_index_status` MAY return `INDEX_UNAVAILABLE` while the new
workspace indexes, per the existing fail-fast requirement.

#### Scenario: listChanged fires with a new first root

- **GIVEN** the server is bound to `/old` via `roots/list`
- **AND** the client declared `roots.listChanged: true`
- **WHEN** the client sends `notifications/roots/list_changed`
- **AND** a subsequent `roots/list` returns `[file:///new]`
- **THEN** the server closes its `/old` snapshot DB handle
- **AND** binds to `/new`
- **AND** triggers background indexing on `/new` if no `.kenn/live/` exists there
- **AND** the rmcp transport keeps accepting tool calls throughout

#### Scenario: listChanged fires with no change

- **GIVEN** the server is bound to `/proj`
- **WHEN** the client sends `notifications/roots/list_changed`
- **AND** a subsequent `roots/list` still returns `/proj` as the first root
- **THEN** the server takes no action

#### Scenario: Client does not support listChanged

- **WHEN** the client declares the `roots` capability without `listChanged: true`
- **THEN** the server registers no notification handler
- **AND** workspace changes require a server restart to take effect
- **AND** the startup log records `listChanged=false`

#### Scenario: `--workspace` flag wins over listChanged

- **GIVEN** the server was launched as `kenn --workspace /home/user/proj mcp`
- **AND** the client declared `roots.listChanged: true`
- **WHEN** the client sends `notifications/roots/list_changed`
- **AND** a subsequent `roots/list` returns `[file:///different]`
- **THEN** the server takes no action — the flag-bound workspace is permanent for this server's lifetime
- **AND** no log line about a rebind is emitted (the notification arrived but was ignored by design)

#### Scenario: `claude-project-dir` source does NOT block listChanged

- **GIVEN** the server was launched as `kenn mcp` (no flag) and bound to `/home/user/proj` via `CLAUDE_PROJECT_DIR`
- **AND** the client declared `roots.listChanged: true`
- **WHEN** the client sends `notifications/roots/list_changed`
- **AND** a subsequent `roots/list` returns `[file:///home/user/other]`
- **THEN** the server rebinds to `/home/user/other` and emits the rebind log line
- **AND** the env-var-tentative bind has no special permanence — only `--workspace` is permanent

### Requirement: Workspace resolution source is logged on every binding change

The server SHALL emit a structured log line on every binding change
— the initial tentative bind, any post-handshake rebind, and any
later `listChanged`-driven rebind — so operators can distinguish
"the user told me" from "the host told me" from "the host is
misbehaving."

The line SHALL include at least:

- `source`: one of `cli-flag`, `claude-project-dir`, `roots-list`, `git-toplevel`, `cwd`.
- `path`: the resolved local filesystem path.
- `listChanged`: boolean, only when `source=roots-list`.
- `ignored_roots`: list of URIs, only when `source=roots-list` and the client returned more than one root.
- `reason`: a short tag explaining a fallback to `git-toplevel` or `cwd`. Values: `client-no-roots-capability`, `client-roots-empty`, `client-roots-non-file`. Always present when the source is `git-toplevel` or `cwd`; never present when the source is `cli-flag` or `roots-list`.

The `reason` field is emitted unconditionally on fallback — `kenn
mcp` always runs the rmcp handshake regardless of who's on the
other end, so there's no clean way to distinguish a real MCP host
from a shell debugger. Shell users see the same field as production
operators; it's accurate either way.

#### Scenario: log shows CLI-flag source

- **WHEN** the server resolves its workspace from a `--workspace` flag
- **THEN** the log line contains `source=cli-flag path=/home/user/proj`
- **AND** the line contains no `reason` field

#### Scenario: log shows roots-list source with degradation note

- **WHEN** the server resolves its workspace from `roots/list` and the client did not declare `listChanged`
- **THEN** the log line contains `source=roots-list path=/home/user/proj listChanged=false`

#### Scenario: log shows git-toplevel fallback with reason

- **WHEN** the server falls through to git-toplevel because the MCP client did not declare the `roots` capability
- **THEN** the log line contains `source=git-toplevel path=/home/user/proj reason=client-no-roots-capability`

#### Scenario: reason field always present on fallback

- **WHEN** the server falls through to `git-toplevel` or `cwd`
- **THEN** the log line contains a `reason` field naming the fallback cause
- **AND** the field's value is one of `client-no-roots-capability`, `client-roots-empty`, `client-roots-non-file`
- **AND** the behavior is the same whether the client is a real MCP host or a shell pretending to be one

### Requirement: Empty-snapshot tools point at config, not silent empty results

When the published snapshot has zero symbols (e.g., a fresh workspace where no language is enabled, or every enabled language found nothing to index), every MCP tool that returns symbol or finding data (e.g., `search_symbols`, `find_symbol`, `find_at_location`, `list_callers`, `list_callees`, `list_usages`, `list_implementers`, `list_overrides`, `list_correspondences`, `list_in_scope`, `list_module_files`, `list_imports`, `get_symbol`, `get_source`, `find_similar`, `search_findings`, `semantic_search`) SHALL return a structured error rather than a silent empty array. Tools that are NOT subject to this rule: the carve-outs `get_index_status` and `get_workspace_overview`, plus all MCP protocol primitives (`initialize`, `tools/list`, `tools/call` dispatch, `notifications/*`) — those continue to operate per their existing contracts. The error is the empty-snapshot dual of the existing *An unresolved entity reference is an error, not an empty result* requirement.

The error SHALL reuse JSON-RPC code `-32002` (the same code kenn-mcp uses today for `IndexUnavailable`/`EmbedderStarting`, since "the index exists but has no data to serve you" belongs to the same service-unavailable family). On the wire, kenn-mcp's existing error envelope places the per-error string code under `data.kenn_subcode` (injected by the server wire layer) and the per-error classifier payload under `data.data`. For an empty-snapshot error this materialises as `data.kenn_subcode = "EMPTY_SNAPSHOT"` plus `data.data = { kind, enabled_languages }`. Agents branch on `data.kenn_subcode` for the error class and on `data.data.kind` / `data.data.enabled_languages` for the classifier — without parsing the human-readable `message`:

- **config-disabled**: every `[language.*].enabled` is `false` in the workspace's `kenn.toml`. `enabled_languages` is the empty array. `message` MUST reference `kenn.toml` and list the strings `csharp`, `rust`, `typescript`, `python` verbatim.
- **configured-but-empty**: at least one language is enabled but the snapshot still has zero symbols. `enabled_languages` lists the enabled language identifiers using the canonical lowercase serialization (`csharp`, `rust`, `typescript`, `python`). `message` MUST identify the case as configured-but-empty AND name the enabled language(s); it MAY include a most-common-cause hint (e.g., "no `.py` files were found"), but the implementation is NOT required to diagnose the actual cause — an honest "snapshot is empty, reason unclear" message naming the enabled language(s) is compliant.

The workspace whose `kenn.toml` is consulted MUST be the workspace resolved by the existing *Workspace resolution follows a five-step priority chain* requirement — not `cwd` — so worktree-bound MCP sessions see the right config.

`get_workspace_overview` MUST succeed in both cases (the empty state itself is information) and its response struct SHALL grow an optional `config_hint` field of shape `{ kind: "config-disabled" | "configured-but-empty", enabled_languages: [..] }`, present only when the snapshot has zero symbols and absent (or `null`) on healthy snapshots.

#### Scenario: MCP query against config-disabled empty snapshot

- **WHEN** `kenn mcp` serves a snapshot with `symbols=0` AND every `[language.*].enabled` is false in `kenn.toml`
- **AND** the agent calls `search_symbols("anything")` (or any other data-returning tool listed in this requirement)
- **THEN** the tool MUST return a structured JSON-RPC error with `code = -32002`, `data.kenn_subcode = "EMPTY_SNAPSHOT"`, and `data.data = { kind: "config-disabled", enabled_languages: [] }`
- **AND** the error `message` MUST reference `kenn.toml` and list the strings `csharp`, `rust`, `typescript`, `python`
- **AND** the error MUST NOT be a generic empty-results array

#### Scenario: MCP query against configured-but-empty snapshot

- **WHEN** the snapshot has `symbols=0` AND `[language.python].enabled = true` (and no other language enabled)
- **AND** the agent calls `find_symbol("Foo")`
- **THEN** the tool MUST return a structured JSON-RPC error with `code = -32002`, `data.kenn_subcode = "EMPTY_SNAPSHOT"`, and `data.data = { kind: "configured-but-empty", enabled_languages: ["python"] }`
- **AND** the error `message` MUST identify the case AND name Python as the enabled language
- **AND** the implementation MAY but is NOT required to add a "no `.py` files" diagnostic — an honest "reason unclear" message is compliant

#### Scenario: get_workspace_overview surfaces config state on empty snapshots

- **WHEN** the snapshot has `symbols=0`
- **THEN** `get_workspace_overview` MUST return successfully
- **AND** the response MUST include a `config_hint` field of shape `{ kind, enabled_languages }` populated per the classification above

#### Scenario: get_workspace_overview omits config_hint on healthy snapshots

- **WHEN** the snapshot has `symbols > 0`
- **THEN** `get_workspace_overview` MUST return successfully
- **AND** the response MUST either omit `config_hint` or set it to `null`

#### Scenario: get_index_status remains the lifecycle-only probe

- **WHEN** the snapshot has `symbols=0` for any reason
- **THEN** `get_index_status` MUST still respond per its existing contract (lifecycle state, snapshot id, indexed_at)
- **AND** MUST NOT return a config-hint error — config diagnosis is the responsibility of the read tools and `get_workspace_overview`

### Requirement: `wait_for_index` blocks until the index settles

The MCP server SHALL expose a `wait_for_index` tool that blocks until
the index reaches a **settled** state or a caller-supplied timeout
elapses, whichever comes first. The index is *settled* when
`get_index_status` would report `state: "ready"` with
`reindex_in_progress: false`, or `state: "failed"`. It is *unsettled*
while `state: "indexing"`, or `state: "ready"` with
`reindex_in_progress: true`.

The tool SHALL accept an optional `timeout_ms` argument. When omitted it
SHALL default to a bounded value (30 000 ms), and the server SHALL clamp
any supplied value to a hard maximum (120 000 ms) so a tool call cannot
block indefinitely.

The response SHALL carry the same status payload `get_index_status`
returns, plus a boolean `timed_out` field: `false` when the tool
returned because the index settled, `true` when it returned because the
timeout elapsed while still unsettled. The tool SHALL NOT return
`INDEX_UNAVAILABLE` in any state.

While waiting, the tool SHALL NOT hold the lifecycle lock across its
wait intervals (it polls), so concurrent tool dispatch is never blocked.

#### Scenario: Returns promptly when already settled

- **GIVEN** the server is `Ready` with no reindex in progress
- **WHEN** the agent calls `wait_for_index { }`
- **THEN** the response returns without waiting
- **AND** `state` is `"ready"` and `timed_out` is `false`

#### Scenario: Blocks through indexing then returns ready

- **GIVEN** the server is `Indexing`
- **WHEN** the agent calls `wait_for_index { "timeout_ms": 60000 }`
- **AND** the pipeline completes and transitions to `Ready` before the
  timeout
- **THEN** the call returns after the transition with `state: "ready"`
  and `timed_out: false`

#### Scenario: Times out while still indexing

- **GIVEN** the server is `Indexing` and does not complete within the
  timeout
- **WHEN** the agent calls `wait_for_index { "timeout_ms": 1000 }`
- **THEN** the call returns after ~1000 ms with `timed_out: true`
- **AND** `state` reflects the still-unsettled state (e.g. `"indexing"`)

#### Scenario: Returns immediately on failed

- **GIVEN** the server is `Failed`
- **WHEN** the agent calls `wait_for_index { }`
- **THEN** the call returns without waiting with `state: "failed"` and
  `timed_out: false`

#### Scenario: Supplied timeout is clamped to the maximum

- **WHEN** the agent calls `wait_for_index { "timeout_ms": 10000000 }`
- **THEN** the effective wait SHALL NOT exceed the hard maximum
  (120 000 ms)

### Requirement: Concurrent tool calls do not serialize behind a lock

No MCP tool handler SHALL hold a lock across an `.await` of slow or
unbounded work such that other concurrent tool calls block behind it.
Read-only tools that need only shared access MUST NOT acquire an
exclusive lock. In particular, the findings store SHALL permit concurrent
read tools (search, get, DAG walks) to proceed in parallel; only mutating
tools serialize.

Every tool handler (except `wait_for_index`, whose blocking is bounded by
its own timeout) SHALL return within a small bounded latency budget on a
Ready server, or with a fast error — never an unbounded wait that also
stalls other tools.

#### Scenario: A slow/long findings read does not stall a concurrent read

- **GIVEN** one findings read tool is executing
- **WHEN** a second findings read tool is called concurrently
- **THEN** the second call proceeds in parallel and is not blocked for the
  full duration of the first

#### Scenario: Only wait_for_index may exceed the budget

- **WHEN** any tool other than `wait_for_index` is called on a Ready
  server
- **THEN** it returns within the bounded budget (or with a fast error),
  not after an unbounded wait

### Requirement: Tool calls are observable via tracing spans and metrics

Each tool call SHALL be wrapped in a `tracing` span carrying the tool
name, whose open→close duration is the call's latency, so a slow or
stuck call is diagnosable from the observability stack — not a profiler.
The same dispatch boundary SHALL emit metrics through a facade (a call
counter and a duration histogram, keyed by tool) so a backend exporter
can be wired without changing the instrumentation. All observability
output goes to stderr or an exporter — **never stdout**, which the stdio
transport reserves for JSON-RPC.

#### Scenario: A completed tool call has a span with name and duration

- **WHEN** a `tools/call` completes
- **THEN** a tracing span identifies the tool by name and its close
  event carries the elapsed duration
- **AND** nothing is written to stdout

#### Scenario: Metrics facade is in place for a future exporter

- **WHEN** a tool call completes
- **THEN** a counter and a duration histogram are recorded for that tool
  through the metrics facade (a no-op until an exporter is installed)

### Requirement: get_workspace_overview reports per-language stats

`get_workspace_overview` SHALL report, all read from the precomputed `stats`
table:
- **per-language** counts (scope `language`): for each language, its
  `symbols`, `files`, and `defs` counts split by `subset`
  (`internal`/`test`/`external`), plus its per-language graph counters
  (`subset='graph'`: `nodes`, `god_nodes`, `communities`, `anchors`) when the
  analysis pass ran;
- **per-manager** package counts (scope `manager`, subsets `internal`/`external`);
- a **whole-graph summary** (`scope='global', subset='graph'`:
  `hierarchy_depth`, `cross_anchor_communities`, `domains`) when present.

The whole-graph summary's `cross_anchor_communities` is the RAW clustering
counter — every community spanning more than one anchor, before any selection —
and SHALL NOT be presented as the workspace's domain count. `domains` is the
EARNED count: the communities that clear the axis floors, which is what the atlas
renders and what a domains query returns. Both SHALL be reported, each named for
what it is, so neither can be mistaken for the other.

NEITHER counter bounds the other, and a consumer SHALL NOT assume an ordering
between them. They range over different candidate sets: a repo one package
dominates reports `cross_anchor_communities = 0` — nothing spans two anchors —
while `domains` stays non-zero, because the axis deliberately keeps within-anchor
clusters for a monolithic library.

These counts come from the build-time `stats` table, not from a live
aggregation on the read path.

The existing scalar fields (`snapshot_id`, `indexed_at`, `file_count`,
`symbol_count`, `packages`, `config_hint`) remain present with their current
meaning; the `languages` field SHALL carry the per-language stat blocks rather
than a bare list of language names. The scalar `symbol_count` / `file_count`
SHALL be the in-code sum of the `symbols` / `files` subset rows (a handful of
integer adds). The overview SHALL perform no **database** aggregation on the
read path — no `SUM`, `count(*)`, `GROUP BY`, or `count_table` query — it only
reshapes the rows `stats()` returns.

#### Scenario: Overview carries per-language breakdown

- **GIVEN** a Ready server over a snapshot indexed in more than one language
- **WHEN** the agent calls `get_workspace_overview`
- **THEN** the response lists each language with its `symbols`/`files`/`defs`
  counts (split by subset) from the `stats` table
- **AND** package counts are reported per manager
- **AND** when the analysis pass ran, a graph-structure summary is included

#### Scenario: Counts are precomputed, not aggregated on read

- **WHEN** `get_workspace_overview` builds its payload
- **THEN** every count comes from a row already in the `stats` table (the
  scalar totals being an in-code sum of the fetched subset rows)
- **AND** the overview runs no `SUM`, `count(*)`, `GROUP BY`, or `count_table`
  query on the read path

#### Scenario: The raw counter cannot be read as the domain count

- **GIVEN** a snapshot whose partition yields more cross-anchor communities than
  clear the domain floors
- **WHEN** the agent calls `get_workspace_overview`
- **THEN** both the raw counter and the earned `domains` count are reported
- **AND** they are distinguishable by name, not by the reader's inference

### Requirement: Atlas axis read tools

The server SHALL expose the atlas's remaining axes as read tools, each answering
from the published snapshot:

- `list_domains` — cross-package domains; optional `domain` argument for one
  domain's spanned packages and central symbols.
- `list_contracts` — cross-package contracts; optional `contract` argument for
  one contract's implementers grouped by package.
- `list_documents` — first-party non-code directories.

Each tool SHALL be a read tool (no index build on the read path) and SHALL return
an empty list rather than an error when its axis is empty for the workspace.

#### Scenario: Domains tool answers from the snapshot

- **GIVEN** a Ready server over a snapshot whose analysis pass ran
- **WHEN** the agent calls `list_domains`
- **THEN** the earned-span domains are returned with their sizes and package spans
- **AND** no clustering is performed on the read path

#### Scenario: Contracts tool answers from the aggregate edges

- **GIVEN** a Ready server over a snapshot with `implements`/`extends_type` edges
- **WHEN** the agent calls `list_contracts`
- **THEN** each first-party interface whose implementers span more than one
  package is returned with its resolvable `pub_id` and package span

#### Scenario: An axis with no results is not an error

- **GIVEN** a workspace whose abstractions are all package-local
- **WHEN** the agent calls `list_contracts`
- **THEN** the response is an empty list and the call succeeds

### Requirement: list_packages reports the package concept's own metadata

`list_packages` SHALL report, for each package, the metadata the atlas package
concept carries and the query previously dropped: the package's root-module doc
(`description`, verbatim, absent when the package has none), its workspace-relative
manifest path (`resource`), its member-file count, and its per-directory file
counts. When a package is subdivided into component sub-areas, the response SHALL
name them.

`description` SHALL be copied verbatim and never synthesized.

#### Scenario: A documented package carries its doc

- **GIVEN** a package whose root module has a doc comment
- **WHEN** the agent calls `list_packages` for it
- **THEN** the response carries that doc verbatim as `description`

#### Scenario: An undocumented package omits the field

- **GIVEN** a package with no root-module doc
- **WHEN** the agent calls `list_packages` for it
- **THEN** no `description` is reported and none is invented

