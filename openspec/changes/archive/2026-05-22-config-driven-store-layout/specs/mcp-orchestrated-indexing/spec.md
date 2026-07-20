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

A non-git workspace carries no `StalenessKey` (there is no `HEAD`), so
a staleness-keyed match is impossible. For such a workspace the startup
decision SHALL degrade to serving the `live` snapshot — the same
behavior as `git_aware_skip = false` — rather than re-indexing on every
startup. `kenn index` SHALL still always re-index a non-git workspace,
since a writer cannot verify whether anything changed.

When the staleness check itself fails (e.g. cannot read snapshot
metadata), the server SHALL conservatively re-index rather than serve
potentially-incorrect data.

#### Scenario: git_aware_skip true and a retained snapshot matches

- **GIVEN** `kenn.toml` sets `staleness.git_aware_skip = true`
- **AND** the workspace's current `StalenessKey` matches the key
  recorded with some retained snapshot
- **WHEN** the MCP server starts
- **THEN** the server opens that snapshot and transitions to `Ready`
  without indexing

#### Scenario: no retained snapshot matches

- **GIVEN** `staleness.git_aware_skip = true`
- **AND** no retained snapshot's recorded `StalenessKey` matches the
  workspace's current key
- **WHEN** the MCP server starts
- **THEN** the server enters `Indexing`

#### Scenario: non-git workspace serves the live snapshot

- **GIVEN** `staleness.git_aware_skip = true`
- **AND** the workspace is not a git repository
- **AND** a `live` snapshot exists
- **WHEN** the MCP server starts
- **THEN** the server opens the `live` snapshot and transitions to
  `Ready` without indexing

#### Scenario: Staleness metadata unreadable

- **GIVEN** a retained snapshot exists but its staleness metadata
  cannot be parsed
- **WHEN** the MCP server starts
- **THEN** the server enters `Indexing` (treats unreadable metadata as
  stale)
