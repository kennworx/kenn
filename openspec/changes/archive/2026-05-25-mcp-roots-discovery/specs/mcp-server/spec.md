## ADDED Requirements

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
