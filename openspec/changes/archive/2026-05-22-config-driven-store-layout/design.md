## Context

`Store` (in `kenn-store/src/layout.rs`) already behaves like a layout object —
it holds one `root` and derives `local_dir()`, `live_path()`, `snapshots_dir()`,
`lock_path()`, etc. from it. But `root` is hardcoded: `Store::open` does
`workspace.join(".kenn")`. Other paths are hardcoded elsewhere —
`findings/store.rs` joins `findings/` and `findings/vectors/`, `driver.rs:373`
joins `scip-{slug}.scip`. `roots::resolve` reads `[workspace] root/store_root/
vectors_root` but `index_workspace` takes a single `workspace_root` and ignores
the split, so the config keys that exist are not actually honored end to end.

Critically, `.kenn/` today mixes two kinds of data: **committed** (`vectors/`,
`findings/<id>.json` — git-tracked) and **derived** (`local/` — code graph and
Lance stores — plus `scip-*.scip`). They have opposite lifecycles but share a
directory, which is why derived intermediates leak past `.gitignore`.

## Goals / Non-Goals

**Goals:**
- One `Layout`, resolved from config, is the single source of every store path.
- The derived store is relocatable — including to a global, cross-branch folder.
- Committed and derived artifacts are explicitly classified and separable.
- Defaults reproduce today's in-repo layout exactly — zero migration.

**Non-Goals:**
- Moving committed artifacts (`vectors/`, `findings/<id>.json`) out of the repo
  — they are version-controlled by design; only their *path* is config-driven,
  defaulting in-repo.
- Changing snapshot/segment file formats or the indexing pipeline.
- Networked / remote stores.
- Making the `live`-based maintenance commands branch-aware under a shared
  `derived_root`. `kenn status` and `kenn rollback` resolve through the `live`
  pointer, which under a shared root reflects whichever branch published last.
  The MCP read path and `kenn index` *are* branch-aware (staleness-keyed);
  per-branch `status` / `rollback` for a shared root is a follow-up change.

## Decisions

### 1. A single `Layout`, resolved once

Introduce `Layout` — the resolved triple plus accessors:

```
Layout {
    source_root:    PathBuf,   // where code lives
    committed_root: PathBuf,   // git-tracked sidecars — ALWAYS <source>/.kenn
    derived_root:   PathBuf,   // throwaway store + intermediates — relocatable
}
```

`committed_root` is a *resolved* field, not a configurable one — it is always
`<source_root>/.kenn`. Only `derived_root` is a config knob. A settable
committed root would let `vectors/` / `findings/` be pointed out of the repo,
silently dropping version-controlled embeddings — exactly the Non-Goal.

Every path comes from accessors — `code_vectors_dir()`, `findings_dir()`,
`findings_vectors_dir()`, `snapshots_dir()`, `live_path()`, `lock_path()`,
`scip_path(slug)`, … No crate joins a literal path segment. `Store` is
reconstructed on top of `Layout` (it already is one in spirit); `roots::resolve`
is subsumed.

### 2. Config: a `[layout]` section — one key

```toml
[layout]
# derived_root = ".kenn/local"   # default; relative paths resolve from source_root
```

`derived_root` is the only knob. It accepts a relative path, an absolute path,
or the keyword `"global"`, which resolves to
`${XDG_CACHE_HOME:-~/.cache}/kenn/<project-id>/`. There is intentionally **no
`committed_root` key** — committed data is always `<source_root>/.kenn` (see
Decision 1). The existing `[workspace] store_root` / `vectors_root` keys are
subsumed (prototype stage — redefined in place, no compatibility shim).

### 3. Committed vs derived — explicit classification

| Artifact | Class | Root |
|---|---|---|
| `vectors/` (code embeddings) | committed | `committed_root` |
| `findings/<id>.json` records | committed | `committed_root` |
| `findings/vectors/` | committed | `committed_root` |
| code-graph + knowledge + findings Lance (`local/`) | derived | `derived_root` |
| `snapshots/`, `live`, `building/`, `runs/`, `index.lock` | derived | `derived_root` |
| `scip-*.scip` indexer intermediates | derived | `derived_root` |

`scip-*.scip` moving under `derived_root` fixes the git-stage leak: with the
default layout it lands in `.kenn/local/`, already gitignored.

### 4. `Layout` threaded through, not re-derived

`index_workspace`, `Store::open`, the findings store, and `driver.rs` take a
`Layout`. This also closes the latent `serve_stdio` bug where `store_root` was
passed as the indexer's `workspace` — there is no longer a single ambiguous
`workspace_root` to mis-pass.

### 5. Shared derived root → snapshot resolution *and* retention by staleness key

A global `derived_root` is shared by every branch/worktree of the repo, so a
single `live` pointer would let branches clobber each other. Two coupled
changes:

**Resolution.** Snapshot resolution changes from "follow `live`" to "among
retained snapshots, **select the one whose recorded staleness key matches
mine**; if none, reindex." `decide_startup_state` already reads recorded
staleness keys — it extends from checking one snapshot to scanning the retained
set. `live` remains only as a "most recent" hint.

A relocated `derived_root` is shared across the repo's branches and worktrees,
so scan-by-key resolution is the *only* thing keeping them apart — and that
needs staleness keys, i.e. `staleness.git_aware_skip = true`. Config resolution
therefore rejects a non-default `derived_root` when `git_aware_skip = false`,
rather than silently collapsing every branch onto the one shared `live`.

A global `derived_root` also supersedes kenn's git-worktree fallback
(`resolve_main_worktree` / `fallback_from_parent_worktree`): every worktree
shares the one store and scan-by-key resolution gives each its own matching
snapshot — strictly better than falling back to the parent worktree's. The
fallback path is unchanged for the default in-repo layout.

**Retention.** Retention must stay bounded, and it cannot be keyed on the
staleness key — that key includes dirty-file hashes, so it changes on every
edit, and "one snapshot per key" would grow without limit. GC therefore keeps
the **`gc_keep` most-recently-*accessed* snapshots**, regardless of key:
snapshot resolution touches the selected snapshot's access time, and GC evicts
least-recently-used. For a single branch this reduces to today's behavior — the
active snapshot stays hot, older ones age out at `gc_keep` depth. Across
branches, the ones you actually work on stay resident; a branch whose snapshot
has aged out simply reindexes on next use. Robust multi-branch reuse is then a
matter of sizing `gc_keep` to the active branch count. The current `live`
target is additionally pinned — never evicted even when cold — so a `rollback`
onto an older snapshot can't later strand `live` on a GC'd directory.

A long-lived reader resolves its snapshot once and then serves for hours
without re-resolving, so its access time goes stale — LRU alone would evict a
snapshot still in use. The `gc_keep` LRU bound therefore governs only the
*unheld* snapshots; a snapshot held by a live reader is exempt. The
cross-process "held by a reader" signal is the reader pin from
`mcp-background-reindex` (which lands after this change); this change's GC
consults it. Until that change lands, LRU GC is pin-unaware exactly as today's
count-based GC is — no regression in the interim.

Together these turn a global `derived_root` into real cross-branch reuse:
switch branch → find the matching prebuilt snapshot → skip indexing.

### 6. Project id for the global path

`<project-id>` is a short hash (xxh3-64) of the canonicalized repository root
path — stable for a repo, distinct between repos, and requiring no git remote.

## Risks / Trade-offs

- **Concurrency on a shared global root** → multiple branches/worktrees, each
  its own `kenn mcp`, now share one derived root. The one-writer `index.lock`
  and the cross-process GC pins from `mcp-background-reindex` cover this — see
  the coordination note below.
- **`index.lock` becomes repo-wide** → with a shared `derived_root` the single
  `index.lock` serializes indexing across *all* branches of the repo, not one
  worktree. Acceptable — indexing is CPU-heavy and concurrent runs would only
  thrash — but it is a behavior change from per-worktree locking.
- **Global cache growth** → snapshots from many branches accumulate under a
  shared root. Bounded by the per-key retention of Decision 5 — one current
  snapshot per branch plus `gc_keep-1` superseded each. A repo with very many
  branches holds many snapshots; that is the intended trade for reuse.
- **A global root is not git-shared** → committed sidecars must stay in-repo
  (a Non-Goal, and `committed_root` is intentionally not a config knob — see
  Decision 1); only derived data globalizes. No correctness risk — derived data
  is always rebuildable.
- **`canonicalize` differences** (symlinked checkouts) could split one repo
  into two project-ids → minor cache duplication, never incorrectness.

### Coordination with `mcp-background-reindex`

Both changes touch the derived store. `mcp-background-reindex` adds a reader
registry (`.kenn/local/readers/`), leans on `index.lock`, and rewrites
`decide_startup_state` for snapshot hot-reload; this change relocates the
derived store and *also* rewrites `decide_startup_state` (resolution by
staleness key). They MUST land in a defined order: **`config-driven-store-layout`
first** — it establishes `Layout` and the staleness-keyed resolution — then
`mcp-background-reindex` builds on it, resolving its reader registry as a
`Layout` derived-store path. If `mcp-background-reindex` lands first, its
hardcoded `.kenn/local/readers/` becomes one more path this change must route
through `Layout`.

## Migration Plan

Additive. Every `[layout]` key defaults to the current in-repo location, so an
existing repo with no `[layout]` section behaves identically. No on-disk
migration. Rollback is a straight revert. The only visible default-layout
change is `scip-*.scip` moving from `.kenn/` to `.kenn/local/` — which only
*removes* a file that should never have been outside `local/`.

## Open Questions

- `[layout]` vs. folding the `derived_root` key into the existing `[workspace]`
  section.
- Should `derived_root = "global"` key the project id on the git remote URL
  (stable across clones) instead of the local repository path?
- Snapshot retention bound — keep `gc_keep` count-based, or make it size-based
  (total bytes) for a shared global root that many branches feed?
