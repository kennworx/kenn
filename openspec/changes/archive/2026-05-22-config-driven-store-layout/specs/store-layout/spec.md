## ADDED Requirements

### Requirement: A single Layout resolves every store path

A single `Layout` value, resolved once from configuration, SHALL be the sole
source of every store path. No component SHALL join store path segments —
`.kenn`, `local/`, `findings/`, `vectors/`, `snapshots/`, `scip-*.scip`, and the
rest — on its own; every path SHALL come from a `Layout` accessor.

#### Scenario: paths come only from Layout

- **WHEN** any component — indexer, store, findings store, or MCP server —
  needs a store path
- **THEN** it obtains that path from a `Layout` accessor
- **AND** no store path segment is hardcoded outside the layout module

### Requirement: Resolved roots; only the derived root is configurable

`Layout` SHALL expose three roots — `source_root`, `committed_root`, and
`derived_root`. `committed_root` SHALL always resolve to `<source_root>/.kenn`
and SHALL NOT be a configuration knob: a settable committed root could place
version-controlled artifacts (`vectors/`, `findings/`) outside the repository.
Only `derived_root` SHALL be configurable, via the `[layout]` section, and SHALL
default to `<committed_root>/local`. A repository with no `[layout]` section
SHALL resolve to exactly the pre-change paths.

#### Scenario: default layout matches current behavior

- **GIVEN** a `kenn.toml` with no `[layout]` section
- **WHEN** the layout is resolved
- **THEN** `committed_root` is `<source_root>/.kenn`
- **AND** `derived_root` is `<source_root>/.kenn/local`

#### Scenario: configured derived root is honored

- **GIVEN** `[layout] derived_root` is set to an explicit path
- **WHEN** the layout is resolved
- **THEN** the code graph, snapshots, and indexer intermediates resolve under
  that path
- **AND** committed artifacts remain under `committed_root`

#### Scenario: committed root is not configurable

- **WHEN** the layout is resolved
- **THEN** `committed_root` is `<source_root>/.kenn` regardless of config
- **AND** no configuration key can relocate the committed artifacts

### Requirement: Every store artifact is classified committed or derived

Each store artifact SHALL be classified as either committed or derived and
resolve under the matching root. Committed artifacts — the code vector sidecar
(`vectors/`), finding records (`findings/<id>.json`), and the finding vector
sidecar (`findings/vectors/`) — SHALL resolve under `committed_root`. Derived
artifacts — the code-graph and Lance stores (`local/`), snapshots, `live`,
`index.lock`, and `scip-*.scip` indexer intermediates — SHALL resolve under
`derived_root`. No derived intermediate SHALL be written outside `derived_root`.

#### Scenario: scip intermediates are derived

- **WHEN** the indexer writes a `scip-*.scip` file
- **THEN** it is written under `derived_root`
- **AND** with the default layout it is therefore inside `.kenn/local/`, which
  the store `.gitignore` already excludes

### Requirement: The derived root may be relocated, including globally

`derived_root` SHALL accept an absolute path outside the repository. It SHALL
also accept the keyword `"global"`, which resolves to an XDG-cache path keyed by
a stable per-repository project id (a hash of the canonicalized repository
root). Relocating `derived_root` SHALL NOT move committed artifacts — they
remain under `committed_root`.

A `derived_root` set away from the in-repo default is shared across the
repository's branches and worktrees, and depends on staleness keys to
disambiguate them. Configuration resolution SHALL reject a non-default
`derived_root` when `staleness.git_aware_skip` is `false`, with an error
explaining the two settings are incompatible — rather than silently degrade
every branch onto a single shared `live` pointer.

#### Scenario: global derived root

- **GIVEN** `[layout] derived_root = "global"`
- **WHEN** the layout is resolved
- **THEN** `derived_root` is an XDG-cache path unique to this repository
- **AND** `committed_root` — `vectors/` and `findings/` — is unchanged and
  in-repo

#### Scenario: relocated derived root with git_aware_skip off is rejected

- **GIVEN** `derived_root` is set away from the in-repo default
- **AND** `staleness.git_aware_skip` is `false`
- **WHEN** configuration is resolved
- **THEN** resolution fails with an error that the two settings are
  incompatible

### Requirement: Snapshot selected by staleness key

Snapshot resolution SHALL select, among the retained snapshots under
`derived_root`, the snapshot whose recorded staleness key matches the
workspace's current staleness key. When no retained snapshot matches, the
caller SHALL reindex. This lets a `derived_root` shared by several branches or
worktrees serve each from its own matching snapshot without a single `live`
pointer clobbering.

#### Scenario: branch switch reuses a matching snapshot

- **GIVEN** a `derived_root` shared by two branches, each with a retained
  snapshot
- **WHEN** a server starts on one branch
- **THEN** it selects the retained snapshot whose staleness key matches that
  branch
- **AND** it does not reindex

#### Scenario: no matching snapshot triggers reindex

- **WHEN** no retained snapshot's staleness key matches the workspace
- **THEN** the caller reindexes rather than serving a mismatched snapshot

### Requirement: Snapshot retention is bounded by recent use

Snapshot garbage collection SHALL retain the `[lifecycle] gc_keep`
most-recently-accessed snapshots among those eligible for eviction, and evict
the rest. Retention SHALL NOT be keyed on the staleness key — that key changes
on every edit, so a per-key policy would grow without bound. Snapshot
resolution SHALL update the selected snapshot's access time, so actively-used
snapshots — across all branches sharing the root — stay resident while stale
ones age out.

A snapshot currently held open by a live reader SHALL NOT be evicted,
regardless of its LRU position — it is exempt from the `gc_keep` bound, which
governs only the unheld snapshots. The cross-process signal for "held by a live
reader" is the reader pin specified by the `mcp-background-reindex` change; this
requirement defers that mechanism to it and states only the exemption.

The current `live` target SHALL likewise never be evicted, regardless of its
LRU position. A `rollback` moves `live` onto an older snapshot; without this
exemption a run of subsequent publishes could age that snapshot out and leave
`live` dangling. Pinning `live` keeps `rollback` and `status` — which resolve
through the `live` pointer — always able to find a target.

#### Scenario: the live target survives GC even when cold

- **GIVEN** `live` points at a snapshot that is outside the `gc_keep`
  most-recently-accessed set (e.g. after a rollback)
- **WHEN** GC runs
- **THEN** the `live` target is retained regardless of its LRU position

#### Scenario: an active branch's snapshot survives another branch's reindex

- **GIVEN** a shared `derived_root` and `gc_keep` large enough for both branches
- **AND** branch A's snapshot was accessed recently
- **WHEN** branch B reindexes and GC runs
- **THEN** branch A's snapshot is retained — it is among the `gc_keep`
  most-recently-accessed
- **AND** branch B's new snapshot is retained

#### Scenario: an aged-out snapshot is reindexed on next use

- **GIVEN** a branch whose snapshot has fallen outside the `gc_keep`
  most-recently-accessed set and been evicted
- **WHEN** that branch is used again
- **THEN** no matching snapshot is found and the workspace reindexes

#### Scenario: default single-branch retention is unchanged

- **GIVEN** a single-branch repo with the default layout
- **WHEN** successive reindexes run
- **THEN** GC retains the `gc_keep` most-recent snapshots, exactly as before
  this change

#### Scenario: a reader-held snapshot is exempt from LRU eviction

- **GIVEN** a snapshot held open by a live reader
- **AND** its LRU position would otherwise place it outside `gc_keep`
- **WHEN** GC runs
- **THEN** the held snapshot is retained
- **AND** the `gc_keep` bound is applied only to the remaining unheld snapshots
