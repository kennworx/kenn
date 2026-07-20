## ADDED Requirements

### Requirement: Local-first read

When opening the store for reads, the query layer SHALL prefer the workspace's local `.kenn/live` if it exists. Only when no local `live` is present SHALL it consult the parent repository (see *Parent fallback*).

#### Scenario: Worktree has its own snapshot

- **WHEN** the workspace is a git linked worktree at `/repo/wt/feature-x`
- **AND** `/repo/wt/feature-x/.kenn/live` exists
- **THEN** reads MUST use the local snapshot
- **AND** the parent repository's `.kenn/` MUST NOT be consulted

### Requirement: Parent fallback for worktrees without an index

When the workspace lacks a local `.kenn/live` and `git rev-parse --show-toplevel` resolves to a different directory whose `.kenn/live` does exist, the query layer SHALL open the parent's `live` snapshot in **read-only** mode. The implementation MUST NOT acquire any write lock on the parent. The implementation MUST mark the read context as "fallback" so consumers can label results accordingly.

#### Scenario: Fresh worktree, parent has snapshot

- **WHEN** a freshly created git worktree at `/repo/wt/feature-x` has no `.kenn/`
- **AND** `/repo/.kenn/live` exists
- **THEN** reads from the worktree MUST be served by `/repo/.kenn/live`
- **AND** the read context MUST be flagged as "fallback from parent"

#### Scenario: Worktree's own indexer running while parent is read

- **WHEN** the worktree is in fallback mode and an indexer run starts locally
- **THEN** reads MUST continue from the parent until the local flip
- **AND** after the local flip, subsequent reads MUST use the local snapshot

### Requirement: No writes to parent from a worktree

A worktree's indexer SHALL write only to its own local `.kenn/`. It MUST NOT take a lock on, write to, or in any way modify the parent repository's `.kenn/`.

#### Scenario: Worktree indexer respects parent

- **WHEN** an indexer runs in a worktree
- **THEN** the parent's `.kenn/index.lock`, `building/`, `live`, and `snapshots/` MUST be untouched

### Requirement: No-snapshot fallback behavior

When neither the local nor any parent `.kenn/live` exists, the query layer SHALL report `Tier-2-unavailable` to consumers. It MUST NOT block; consumers decide how to render this state.

#### Scenario: Fresh clone with no index

- **WHEN** a brand-new clone has neither local nor parent `.kenn/`
- **THEN** any read attempt MUST return a structured `Tier-2-unavailable` status
- **AND** the structured status MUST hint that the user should run `kenn index`

### Requirement: Parent-resolution is git-driven

The store SHALL identify the parent repository by querying git for the main worktree path (equivalent to `git worktree list --porcelain`, finding the entry that is not a linked worktree, or `git rev-parse --git-common-dir` resolved to its parent). The resolution MUST NOT assume any conventional path layout.

#### Scenario: Worktree at unconventional path

- **WHEN** `git worktree list` reports the main worktree at `/repo` and the current worktree at `/elsewhere/feature-x`
- **THEN** parent resolution MUST identify `/repo` as the fallback target
- **AND** the resolution MUST NOT depend on any path-pattern heuristic
