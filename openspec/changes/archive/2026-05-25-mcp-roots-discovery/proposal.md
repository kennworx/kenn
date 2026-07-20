## Why

Kenn-mcp resolves its workspace through a global `--workspace`
fallback chain (`crates/kenn-cli/src/main.rs:24-30`):

```
1. --workspace <path>           explicit flag
2. git rev-parse --show-toplevel  the parent git repo of cwd
3. cwd                            last resort
```

That chain assumes the server was *launched by a human inside the
project*. It breaks when an MCP host (Claude Code, Cursor, Zed)
auto-launches `kenn mcp` from a plugin config or a global server
registry:

- The launching process's cwd is the editor binary's directory, or
  `/`, or wherever the host was spawned from — **not** the user's
  project.
- `git rev-parse --show-toplevel` from that cwd either fails or
  resolves to some unrelated repo.
- Nothing in the chain consults the host's notion of "the workspace
  the user is in."

The MCP spec (2025-11-25, `client/roots`) standardizes exactly that:
clients declare a `roots` capability and the server pulls the
workspace via `roots/list`. With `roots.listChanged: true`, the
server gets `notifications/roots/list_changed` whenever the user
adds/removes a folder.

For auto-launched servers, roots is the *only* reliable source.
Without it kenn-mcp ends up bound to the wrong directory — usually
producing the "indexing in progress" / empty-results UX that looks
like a kenn bug but is actually a workspace-resolution bug.

### Reality check: Claude Code today

Per <https://github.com/anthropics/claude-code/issues/31893> (closed,
"not planned"), Claude Code currently:

- **Does respond to `roots/list`** — incoming requests work.
- **Does NOT emit `notifications/roots/list_changed`** — `/add-dir`
  silently fails to notify servers (issue #26663).
- Sampling, elicitation, progress, and message notifications are
  firmly absent.

So this change targets a partial-compliance host. The server SHALL
attempt `roots/list` when the client declares the capability, SHALL
subscribe to `listChanged` when it's declared, but MUST function
correctly with the static set obtained at `initialize` time when
notifications never fire. With Claude Code, that means: pick up
roots once on startup, use them until the server restarts — the
graceful-degradation mode the spec anticipates.

Reference: <https://modelcontextprotocol.io/specification/2025-11-25/client/roots>

## What Changes

### Insert two new sources into the workspace-resolution chain

The new chain, in priority order:

```
1. --workspace <path>                 explicit operator intent
2. CLAUDE_PROJECT_DIR env var         Claude Code's pre-handshake spawn signal
3. roots/list (post-handshake)        protocol-clean host workspace
4. git rev-parse --show-toplevel      legacy fallback
5. cwd                                last resort
```

Steps 1, 4, 5 already exist. This change adds steps 2 and 3.

Source (2) is Claude-Code-specific: Claude Code sets
`CLAUDE_PROJECT_DIR` in every MCP subprocess's environment at spawn
time, pointing at the workspace it considers active. Confirmed via
the `debug_env` MCP tool against Claude Code 2.1.148. Cursor and Zed
do not set this variable; for those hosts, step 3 is the primary
path.

Source (3) is the protocol-blessed answer per the 2025-11-25 MCP
spec. Available to any host that declares the `roots` capability.

When both (2) and (3) are available and agree, (2) wins by virtue of
ordering (no wasted post-handshake rebind). When they disagree —
extremely unlikely — (3) takes precedence because the post-handshake
rebind machinery (D6) overwrites the tentative bind.

Why between, not first: an explicit `--workspace` flag is operator
intent and overrides everything. A roots response is host intent;
when both are available we honor the operator. Git-toplevel and cwd
become the fallback for non-MCP-host launches (e.g. running `kenn
mcp` manually from inside a checkout for debugging) — exactly today's
behavior preserved for that case.

### Detect "untrustworthy cwd" and surface it

When the chain falls through to git-toplevel or cwd *while operating
as an MCP server*, that's almost always a misconfiguration: the host
didn't set `CLAUDE_PROJECT_DIR` AND didn't provide roots. The
startup log line MUST clearly flag this so a user debugging "kenn
returns nothing" sees the root cause immediately:

```
roots discovery: source=cli-flag           path=/home/user/proj
roots discovery: source=claude-project-dir path=/home/user/proj
roots discovery: source=roots-list         path=/home/user/proj  listChanged=true
roots discovery: source=git-toplevel       path=/home/user/proj  reason=client-no-roots-capability
roots discovery: source=cwd                path=/                reason=client-no-roots-capability
```

The last two are not failures (the server still runs), but the
`reason` field tells the operator "this host doesn't speak the
modern protocol, here's why we fell through."

### Tentative early bind + late rebind

Roots resolution needs the rmcp `Peer`, which exists only after the
`initialize` handshake. But `ServerState` requires a `Layout` (and
therefore a workspace path) at construction time, *before* the
handshake. To break the cycle:

- Pre-handshake: bind tentatively using the highest-priority source
  available without a peer — flag → git-toplevel → cwd. Indexing on
  the tentative workspace kicks off in the background.
- Post-handshake (in `on_initialized`): if the client declared
  `roots`, call `roots/list`. If the returned first root differs
  from the tentatively-bound path, **rebind**: abort in-flight
  indexing, swap the layout, re-run the startup decision against
  the new workspace.

The mental model: *the first `roots/list` call is just a
`listChanged` notification fired once at startup.* Both paths share
the same rebind code. Implementation detail in design.md (D6, D7).

### listChanged subscription (when available)

If the client declares `roots.listChanged: true`, kenn-mcp registers
a handler for `notifications/roots/list_changed`. On receipt it
re-issues `roots/list` and, if the resolved first root URI changed,
rebinds through the same machinery as the initial post-handshake
resolution.

If the client declares `roots` but not `listChanged`, the server
uses the initialize-time list and never re-fetches. This is the
Claude Code path today.

If the operator passed `--workspace`, the flag wins permanently:
neither the initial post-handshake `roots/list` nor later
`listChanged` notifications can override it.

### Single-root constraint

If `roots/list` returns more than one root, the server uses the
first `file://` root and logs the rest as ignored. Kenn's storage
model is "one workspace, one index" — multi-root indexing is a
separate change requiring storage-layout work. The startup log names
the ignored roots so the operator can fix it if the wrong one was
picked.

Non-`file://` URIs (e.g. `vscode-vfs://`) are skipped with a logged
reason. The 2025-11-25 spec mandates `file://`.

### Capability negotiation surface

Kenn-mcp declares no new server-side capability — this is a
client-capability consumer change. The rmcp library handles the
`initialize` handshake; we hook the post-handshake step where the
client's declared capabilities are visible.

### Out of scope

- **Multi-root indexing**: see above.
- **`/add-dir`-style workspace changes from Claude Code**: until
  issue #26663 is fixed, the listChanged path is dead code with
  Claude Code as the host. We wire it for Cursor / Zed / future
  Claude Code.

## Capabilities

### Modified Capabilities

- `mcp-server`: gains `roots/list` as a workspace-resolution source
  between the explicit `--workspace` flag and the git-toplevel
  fallback. No change to existing operator invocations.
