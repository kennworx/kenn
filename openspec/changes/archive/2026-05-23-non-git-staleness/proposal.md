## Why

kenn's reindex-skip optimization and its staleness-keyed snapshot
resolution are keyed entirely on git. `StalenessKey` is
`(HEAD commit, dirty-file xxhashes)`; `compute_staleness_key` returns an
empty, never-matching key when the workspace is not a git repository.

The `config-driven-store-layout` change made non-git workspaces
*usable* — `decide_startup_state` degrades to "serve `live`" instead of
erroring — but a non-git project still gets **no staleness detection at
all**: `kenn index` always fully re-indexes it, the MCP server never
notices the workspace changed, and the staleness-keyed snapshot
resolution (including cross-branch reuse on a shared `derived_root`)
does not apply. Non-git support today is "doesn't break", not "works".

## What Changes

- `StalenessKey` gains a **non-git representation** — a fingerprint of
  the source tree. `compute_staleness_key` produces the git key
  `(HEAD, dirty hashes)` in a git repo, and a tree-fingerprint key
  otherwise.
- The tree fingerprint is an `xxh3-64` over every non-excluded source
  file's `(workspace-relative path, mtime, size)` — a `stat`-only walk,
  never a content read, so it stays cheap enough for a "should I skip"
  gate.
- The fingerprint walk skips a fixed set of directory names
  (`node_modules`, `target`, `bin`, `obj`, `.git`, `.kenn`) — not the
  configurable `[exclude] globs`, which would ripple a config argument
  through every staleness caller. A config-excluded file that changes
  therefore triggers at most one redundant (always-safe) reindex.
- `StalenessKey::matches` compares whichever representation both keys
  carry; a git key and a tree key never match.
- With a real key for non-git workspaces, the existing machinery —
  `kenn index` skip, `decide_startup_state` scan-by-key,
  `embed_pending` / `reembed` resolution, and shared-`derived_root`
  snapshot reuse — applies to them uniformly. The non-git "degrade to
  serve `live`" branch added by `config-driven-store-layout` is removed.

## Capabilities

### New Capabilities

- `workspace-staleness`: the staleness-key contract — its git form
  `(HEAD, dirty-file hashes)`, its non-git tree-fingerprint form, how
  `compute_staleness_key` produces each, and the `matches` equivalence.

### Modified Capabilities

- `mcp-orchestrated-indexing`: the snapshot-freshness decision no longer
  special-cases non-git workspaces — they carry a real (fingerprint)
  staleness key, so the scan-by-key path applies to them exactly as to
  git workspaces. This supersedes the non-git "serve `live`" degrade
  introduced by `config-driven-store-layout`.

## Impact

- `kenn-store` — `staleness.rs`: the `StalenessKey` shape,
  `compute_staleness_key`, `matches`, and a new `stat`-based tree-walk
  fingerprint with its fixed skip-list.
- `decide_startup_state` (`lifecycle.rs`) — the non-git `follow_live`
  branch from `config-driven-store-layout` is removed; non-git flows
  through the same scan-by-key path. No signature change — the
  fingerprint walk needs no config.
- No on-disk migration. Snapshots record whatever key shape was current
  when they were built; a snapshot from before this change simply never
  matches a tree key and triggers one reindex.
- Lands **after** `config-driven-store-layout` — it removes that
  change's non-git degrade.
- The `staleness.git_aware_skip` setting name becomes a slight misnomer
  once staleness works without git — see design Open Questions.
