## MODIFIED Requirements

### Requirement: Snapshot freshness check reuses existing staleness machinery

The startup decision (run indexing vs. skip) SHALL use the
`compute_staleness_key` and `StalenessKey::matches` functions from
`kenn-store::staleness`. The decision SHALL honor the
`staleness.git_aware_skip` setting in `kenn.toml`.

The decision SHALL consider every retained snapshot under the derived
store, not only the one `live` points at: the server SHALL select the
retained snapshot whose recorded `StalenessKey` matches the
workspace's current key, and SHALL skip indexing only when such a
snapshot exists. When no retained snapshot matches, the server SHALL
re-index. This lets a derived store shared across branches or
worktrees serve each from its own matching snapshot.

This SHALL apply uniformly whether or not the workspace is a git
repository. A non-git workspace carries a tree-fingerprint
`StalenessKey` (see the `workspace-staleness` capability), so the
startup decision SHALL resolve it through the same scan-by-key path it
uses for a git workspace — it SHALL NOT special-case a non-git
workspace. (This supersedes the interim non-git "serve `live`" degrade
from `config-driven-store-layout`, which existed only because non-git
workspaces previously had no usable key.)

When the staleness check itself fails (e.g. cannot read snapshot
metadata, or the workspace fingerprint cannot be computed), the server
SHALL conservatively re-index rather than serve potentially-incorrect
data.

#### Scenario: git_aware_skip true and a retained snapshot matches

- **GIVEN** `kenn.toml` sets `staleness.git_aware_skip = true`
- **AND** the workspace's current `StalenessKey` matches the key
  recorded with some retained snapshot
- **WHEN** the MCP server starts
- **THEN** the server opens that snapshot and transitions to `Ready`
  without indexing

#### Scenario: non-git workspace resolves by tree fingerprint

- **GIVEN** `staleness.git_aware_skip = true`
- **AND** a non-git workspace whose tree-fingerprint key matches the key
  recorded with some retained snapshot
- **WHEN** the MCP server starts
- **THEN** the server opens that snapshot and transitions to `Ready`
  without indexing

#### Scenario: non-git workspace changed since its last index

- **GIVEN** a non-git workspace whose source tree has changed since the
  retained snapshot was built
- **WHEN** the MCP server starts
- **THEN** no retained snapshot's tree fingerprint matches, and the
  server re-indexes

#### Scenario: staleness metadata unreadable

- **GIVEN** a retained snapshot exists but its staleness metadata
  cannot be parsed
- **WHEN** the MCP server starts
- **THEN** the server re-indexes (treats unreadable metadata as stale)
