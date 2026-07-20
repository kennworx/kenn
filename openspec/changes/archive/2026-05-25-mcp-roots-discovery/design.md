# Design — mcp-roots-discovery

## Decisions

### D1: Two new sources slot between `--workspace` and git-toplevel

The existing chain is `flag → git-toplevel → cwd`
(`main.rs:24-30`). It becomes:

```
1. --workspace <path>                 operator intent
2. CLAUDE_PROJECT_DIR env var         Claude Code pre-handshake spawn signal
3. roots/list (when supported)        host intent, post-handshake
4. git rev-parse --show-toplevel      legacy fallback
5. cwd                                last resort
```

Rationale:

- **The flag stays for tests.** No production MCP launch passes
  `--workspace` to `kenn mcp` — Claude Code / Cursor / Zed have no
  way to fill it in. But the test harness relies on it heavily
  (`crates/kenn-mcp/tests/lifecycle.rs:109`, `tests/end_to_end.rs:22`,
  `crates/kenn-cli/tests/cli_smoke.rs:16`) to point a fresh `kenn mcp`
  at a `TempDir` workspace without polluting cwd. Dropping it would
  force a test rewrite for no production benefit.
- Roots is more authoritative than the launching process's cwd:
  Claude Code knows which project the user is in; the cwd of the
  spawned process does not. When both are available with no explicit
  flag, the host wins.
- Git-toplevel and cwd remain as fallbacks so manual `kenn mcp`
  invocations from inside a checkout keep working unchanged for
  debug / dev workflows.

Alternative considered: *put roots first, ahead of `--workspace`*.
Rejected — the test harness sets the flag intentionally and would
break if a test fixture's MCP client happened to report roots.

### D2: First-root-wins on multi-root response

`roots/list` may return any number of roots. Kenn's storage backend
binds to a single workspace (`.kenn/live/` is per-workspace).
Supporting N roots properly means N parallel snapshot DBs or a
unioned index — both larger than this change.

For v1: the server uses `roots[0]` (after filtering out non-`file://`
URIs) and emits a startup log line naming the ignored roots with
their URIs. The agent can then ask the user to narrow the workspace
if the wrong root was picked.

Alternatives considered:

- *Hash all roots into a stable index path.* Splits the index when a
  root is added/removed and breaks invalidation. Rejected.
- *Refuse to start with >1 root.* Hostile to the common case (most
  editors expose one workspace folder); a user with an
  accidentally-configured second root would lose the server.
  Rejected.
- *Pick the root containing the most code.* Tempting but slow at
  startup (must walk every root) and surprising when wrong.
  Rejected.

First-root-wins is the simplest defensible choice. The log line
gives the user the information to fix it.

### D3: Re-scope on listChanged is a full rebind, not an incremental update

When `listChanged` fires and the first root URI changed, the server
re-binds — closes its current snapshot DB handle, opens the new
workspace's DB (or triggers indexing if `.kenn/live/` doesn't exist
yet). This may include a brief `Indexing` state on the new
workspace.

In-flight tool calls against the OLD workspace are allowed to
complete; new calls land on the NEW workspace. We do NOT block the
transport during rebind.

Rationale: trying to incrementally migrate symbols across workspaces
is a much bigger change with no clear payoff. The user's mental
model ("I switched workspaces; kenn re-indexed") matches the
behavior.

### D4: No new server-declared capability

Roots are a *client* capability — the client declares it, the server
consumes it. Kenn-mcp doesn't need to advertise anything in its
server-side capabilities for this to work.

### D5: Reject non-`file://` URIs

The 2025-11-25 spec mandates `file://` URIs for roots. Any other
scheme (e.g. `vscode-vfs://`, `git://`) is skipped at the
roots-consumption step with a structured warning log. The next
`file://` URI in the returned list becomes the candidate. If no
`file://` root is found, resolution falls through to git-toplevel /
cwd.

### D6: Tentative early bind + late rebind from `on_initialized`

Roots resolution needs the rmcp `Peer`, which exists only after the
MCP handshake. But `ServerState` currently requires a `Layout` —
which requires a workspace path — at construction time, *before* the
handshake. Circular.

Two ways to break the cycle:

- **A. Late-bound layout.** Make `ServerState.layout` an empty cell.
  Override `on_initialized`, call `peer.list_roots()`, build the
  Layout, set the cell, then start indexing. Every tool that
  touches `layout` has to wait or return `INDEX_UNAVAILABLE`.
- **B. Tentative early bind + late rebind.** Keep the existing
  `main.rs::resolve_workspace()` flow (git-toplevel / cwd) as a
  *tentative* binding. Start serving, start indexing on the
  tentative workspace. In `on_initialized`, if the client supports
  `roots` and `roots/list` returns a different path, trigger the
  same rebind path used for `listChanged`.

**Picking B.** Reasons:

- The rebind machinery is on the task list anyway (abort in-flight
  indexing, switch workspace, restart). Both options use it.
- A requires touching every tool method to handle the unbound
  state. B touches only the post-`on_initialized` path.
- Mental model is cleaner: *`roots/list` is just a `listChanged`
  notification fired once at startup*. One code path handles both.
- Wasted-work cost is bounded: if the tentative workspace is wrong
  (plugin-launched, cwd=`/`), kenn's indexer fails fast — no source
  files — and the rebind picks up the real workspace via existing
  abort/restart paths.

If wasted indexing turns out to bite (e.g. a large repo wrongly
guessed by git-toplevel), we can upgrade to A later. For v1, B.

### D7: `ServerState.layout` becomes `ArcSwap<Layout>`

Rebind needs to swap the active layout under live readers. Today
`state.layout: kenn_store::Layout` is a plain field (`tools.rs:35`).

- `OnceLock` doesn't work — it's set-once; rebind needs re-set.
- `RwLock<Layout>`: works but adds contention on the read-hot path.
- `ArcSwap<Layout>`: lock-free reads, cheap swaps, exactly the
  read-mostly pattern we have.

**Picking `ArcSwap`.** Every reader becomes `state.layout.load()`;
the rebind path is `state.layout.store(Arc::new(new_layout))`.

Two collateral signature changes fall out:

- `start_background_indexing(state, layout, config, peer)`
  (`indexing.rs:45`) currently takes layout as a parameter
  *separate* from the one inside state. After D7, the inner layout
  is the source of truth. Drop the parameter; read from state.
- `start_snapshot_poll_task(state, layout, git_aware_skip)`
  (`indexing.rs:442`) same problem. Same fix.

This is a structural change with no behavior delta — done as its
own commit (tasks 2.1–2.5) before the roots-specific work lands.

### D8: `CLAUDE_PROJECT_DIR` is the Claude-Code-specific pre-handshake source

Per the official Claude Code MCP docs and confirmed via the
`debug_env` MCP tool against Claude Code 2.1.148, Claude Code sets
the `CLAUDE_PROJECT_DIR` environment variable on every MCP
subprocess at spawn time. The value is the absolute path to the
project root that Claude Code considers active.

Why it slots ahead of `roots/list`:

- It's available *pre-handshake* — no rmcp roundtrip, no waiting
  for `initialize` to complete. The tentative bind is correct from
  the first instant.
- For Claude Code workflows, the env var matches what `roots/list`
  would return anyway. Using the env var first eliminates the
  "wasted indexing run on git-toplevel/cwd then rebind" path that
  D6 (Option B) accepts as a cost.
- Cursor and Zed do not set this variable — those hosts still rely
  on `roots/list` as their primary signal.

If both `CLAUDE_PROJECT_DIR` and `roots/list` are present:

- When they agree, no rebind happens — the post-handshake check
  sees the workspace already matches the tentative bind. One log
  line, source=`claude-project-dir` (the env source wins by
  position).
- When they disagree (theoretically possible if Claude Code's
  workspace shifts between spawn and the handshake — never
  observed in practice), the post-handshake rebind path overwrites
  the tentative bind, and the final log line shows
  source=`roots-list`. The protocol answer outranks the
  spawn-time hint when they conflict.

Empty / missing-directory check: the env var value MUST point at an
existing local directory. If `CLAUDE_PROJECT_DIR` is set but the
path doesn't exist (rare — Claude Code shouldn't do this), kenn-mcp
SHALL log the rejection and fall through to step 3.

### D9: Log the resolution source with a `reason` field on fallback

When resolution falls through to git-toplevel or cwd, the operator
needs to know *why*. Three plausible reasons:

- `client-no-roots-capability`: client didn't declare `roots` at all.
- `client-roots-empty`: client declared the capability but
  `roots/list` returned `[]`.
- `client-roots-non-file`: only non-`file://` URIs returned.

The startup log line MUST include `reason=<one-of-above>` when the
source is `git-toplevel` or `cwd` *and* the server is operating in
MCP mode. This converts "kenn returns nothing" support cases into a
single-line diagnostic.

## Risks

### R1: Claude Code never fires listChanged

Per issue #31893 (closed, "not planned"), Claude Code does not emit
`notifications/roots/list_changed`. With Claude Code as the host:

- The initial `roots/list` works → roots discovery succeeds.
- `/add-dir` in Claude Code silently doesn't notify → server stays
  bound to the initial root.
- User must restart the server to pick up workspace changes.

Mitigation: document this in the user-facing docs and the kenn-mcp
startup log (`listChanged=false`). The Cursor / Zed paths will work
correctly when those hosts wire listChanged.

### R2: rmcp's `roots/list` call API (resolved)

rmcp 1.6 — the version pinned in `crates/kenn-mcp/Cargo.toml:26` —
exposes the full surface this change needs:

- `Peer<RoleServer>::list_roots() -> Result<ListRootsResult, ServiceError>`
  for the server→client request.
- `ServerHandler::on_roots_list_changed(...)` as the overridable
  handler for the inbound notification (default no-op).
- `ServerHandler::initialize(InitializeRequestParams)` exposes the
  client's `ClientCapabilities`, including the `roots.listChanged`
  bit.

No upstream PR needed. Implementation can proceed directly.

### R3: Workspace switch during in-flight indexing

If `listChanged` fires while the server is in `Indexing` state on
the old workspace:

- Option A: abort the in-flight indexing, re-bind, start indexing
  the new workspace.
- Option B: queue the rebind until indexing completes.

Picking A: aborting is safer than holding open a half-indexed DB.
The new workspace's indexing path is the same as a fresh start; the
abort uses the existing indexing-orchestrator interrupt path.

### R4: Manual `kenn mcp` in a non-MCP context

If someone runs `kenn mcp` from a shell to debug (no real MCP host
on the other end), the chain falls through to git-toplevel / cwd —
today's behavior. The `reason` field on the log line still fires
(`client-no-roots-capability` etc.), which from the shell user's
view reads as "the imaginary MCP client I'm pretending to be didn't
declare roots." That's accurate and not misleading.

Detection is intentionally NOT attempted: `kenn mcp` always runs the
rmcp handshake — there's no "no initialize" code path. Trying to
distinguish "real MCP host that's buggy" from "shell user piping
JSON" would require sniffing for known-host fingerprints in the
client info, which is fragile. We keep the rule simple: when
binding via git-toplevel or cwd, emit the `reason` field naming why
we fell through. Shell debuggers see the same line as production —
the field is informative either way.

## Migration

### Existing invocations

| Invocation | Behavior |
|---|---|
| `kenn --workspace /foo mcp` | unchanged (step 1 wins) |
| `cd /foo && kenn mcp` (in a repo, no MCP host) | unchanged when no `CLAUDE_PROJECT_DIR` / no `roots` (step 4 wins) |
| `cd /foo && kenn mcp` (from Claude Code) | new: step 2 (`CLAUDE_PROJECT_DIR`) wins immediately; no wasted cwd indexing |
| `cd /foo && kenn mcp` (from Cursor/Zed) | new: step 3 (`roots/list`) wins post-handshake; tentative bind via git-toplevel/cwd happens first |
| `kenn mcp` from a plugin config (no cwd context, no flag) | new: step 2 wins (Claude Code) or step 3 wins (other hosts) where today we'd misbind via cwd |

Two real behavior changes for existing users:

- **Claude Code spawn** now binds to `CLAUDE_PROJECT_DIR`
  immediately. Previously git-toplevel of the launching cwd —
  often the editor's directory, not the project. Net effect: kenn
  finds the right workspace from the first instant for Claude
  Code, no rebind needed.
- **Cursor/Zed spawn** still gets the right workspace, but via
  `roots/list` after the handshake (one rebind). Cost: brief
  tentative indexing of whatever git-toplevel/cwd points at; the
  rebind aborts it cleanly via R3's path.

If the operator really wants a different workspace than the host
proposes, they can pass `--workspace` explicitly (step 1 wins
permanently).

### Telemetry / log line

The startup log should clearly state the source:

```
roots discovery: source=cli-flag       path=/home/user/proj
roots discovery: source=roots-list     path=/home/user/proj  listChanged=true  ignored_roots=[]
roots discovery: source=roots-list     path=/home/user/proj  listChanged=false
roots discovery: source=git-toplevel   path=/home/user/proj  reason=client-no-roots-capability
roots discovery: source=cwd            path=/                reason=client-no-roots-capability
```

The lower-priority sources gain a `reason` field when MCP-launched,
making "kenn auto-launched and bound to the wrong place" a
one-grep diagnostic.
