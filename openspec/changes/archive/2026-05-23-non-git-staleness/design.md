## Context

`kenn-store/src/staleness.rs` defines `StalenessKey { git_head:
Option<String>, dirty_files: Vec<DirtyFile> }` and `compute_staleness_key`
— it runs `git rev-parse HEAD` and `git status --porcelain`, hashing only
the files git reports dirty. For a non-git workspace `git_head` is `None`
and `StalenessKey::matches` short-circuits to `false` ("non-git → always
run").

`config-driven-store-layout` worked around the fallout — `decide_startup_state`
degrades to "serve `live`" for a non-git workspace rather than reindexing
forever or erroring — but that is a degradation, not support: a non-git
project gets no incremental skip and no staleness-keyed resolution.

This change gives a non-git workspace a *real* staleness key, so every
existing staleness consumer works for it unchanged.

## Goals / Non-Goals

**Goals:**
- A non-git workspace produces a `StalenessKey` that changes when the
  source tree changes and is stable when it does not.
- `kenn index` skip, `decide_startup_state` scan-by-key, and
  `embed_pending` / `reembed` resolution work for non-git workspaces by
  the same code path as for git workspaces.
- No new signature threading — `compute_staleness_key` stays `&Path`.

**Non-Goals:**
- Content-hashing the source tree. The fingerprint is `stat`-based
  (mtime + size); a deliberate edit-then-restore-mtime-and-size is not
  detected. Git's own index uses mtime for the same reason, and kenn's
  findings-store `BuildStamp` already gates on mtime.
- Honoring `[exclude] globs` in the fingerprint walk (see Decision 3).
- A file-watcher or any push-based change detection — this is the
  pull-time "has it changed since the last index" gate only.
- Renaming `staleness.git_aware_skip` (see Open Questions).

## Decisions

### 1. `StalenessKey` becomes an enum

```
enum StalenessKey {
    /// Git workspace — HEAD commit + dirty-file hashes (today's key).
    Git { head: String, dirty_files: Vec<DirtyFile> },
    /// Non-git workspace — a fingerprint of the source tree.
    Tree { fingerprint: u64 },
    /// Neither resolvable (not git, tree walk failed) — never matches.
    Unknown,
}
```

An enum makes `matches` total and the two forms unmixable by
construction; the alternative — keeping the struct and adding an
`Option<u64>` fingerprint field — leaves "git key XOR tree key" an
unenforced invariant. The serialized form (recorded in each snapshot's
`meta.json` `staleness_key`) changes; this is acceptable at the current
prototype stage — no compatibility shim, a pre-change snapshot simply
never matches and triggers one reindex.

### 2. `matches` compares like-for-like

`Git` matches `Git` iff `head` and `dirty_files` are equal (today's
rule). `Tree` matches `Tree` iff `fingerprint` is equal. Every mixed or
`Unknown` pairing is `false` — conservative: a mismatch only ever costs
one redundant, always-safe reindex.

### 3. The tree fingerprint — `stat`-only, fixed skip-list

`compute_staleness_key`, when `git rev-parse HEAD` fails, walks the
workspace depth-first and folds each file into an `xxh3-64` hasher as
`(workspace-relative path, mtime_nanos, size)`. Files are visited in a
deterministic (sorted) order so the digest is stable.

**`stat`-only, not content.** Hashing file *contents* every staleness
check is `O(repo bytes)` — too slow for a gate whose entire purpose is
to *avoid* work. `(mtime, size)` is one `stat` per file, `O(file
count)`, and catches every realistic edit (an edit bumps mtime; most
also change size).

**Fixed skip-list, not `[exclude] globs`.** The walk skips a fixed set
of directory leaf names — `node_modules`, `target`, `bin`, `obj`,
`.git`, `.kenn`. It deliberately does *not* consult the configurable
`[exclude] globs`: doing so would thread a `GlobSet` (or `&Config`)
through `compute_staleness_key` and `decide_startup_state` and every
caller. The cost of the fixed list is bounded and safe — a file that
`[exclude] globs` excludes but the fixed list does not will, if edited,
trigger one extra reindex. Over-reindexing is always correct; it is only
sub-optimal. `.kenn` MUST be skipped — otherwise publishing a snapshot
(which writes under `.kenn/local/`) would itself change the fingerprint.

The walk MUST NOT follow directory symlinks (or MUST guard against
cycles): `compute_staleness_key` runs on every staleness check, and a
symlink loop would hang it.

### 4. No signature threading

Because the walk needs no config (Decision 3), `compute_staleness_key`
keeps its `&Path` signature and `decide_startup_state` is unchanged
apart from removing the non-git branch. `index_workspace`, `cmd_index`,
and the MCP startup path are untouched.

### 5. `decide_startup_state` drops the non-git special case

`config-driven-store-layout` added: "if the current key has no
`git_head`, degrade to `follow_live`." With this change a non-git
workspace's current key is `Tree { .. }`, not an empty key — the
scan-by-key path handles it directly (it will `matches` a snapshot
recorded with the same `Tree` fingerprint). The degrade branch is
removed; `Unknown` (tree walk failed) still falls through to `Reindex`,
which is correct.

`kenn index` no longer needs its `staleness.git_head.is_some()` skip
guard (also from `config-driven-store-layout`): a non-git workspace now
has a matchable key, so an unchanged non-git workspace legitimately
skips. The guard is removed; the skip reverts to "`Skip` ⇒ skip".

## Risks / Trade-offs

- **`stat` granularity** — a filesystem with coarse mtime, or an edit
  that restores both mtime and size, is missed. Pathological; `--force`
  and a normal edit (which moves mtime) both cover it.
- **Walk cost on a huge non-git tree** — `O(file count)` `stat`s per
  staleness check. For a very large tree this is slower than git's
  index-backed `git status`, but still well under the cost of the
  reindex it gates. Bounded by the fixed skip-list.
- **`[exclude] globs` not applied** — the fingerprint covers files that
  `[exclude] globs` would exclude. Editing one triggers a redundant
  (always-safe) reindex. The sharper case: a non-git project whose
  *churning* output lands in a directory outside the fixed skip-list
  (a build dir not named `node_modules` / `target` / `bin` / `obj`) —
  there the fingerprint changes constantly and the gate effectively
  never skips. That is no *regression* (it degrades to today's
  always-reindex), but it is no *benefit* either. Projects with standard
  layouts are unaffected. If it bites in practice, the remedy is to
  thread the config excludes (the signature ripple Decision 3 avoids)
  or to widen the fixed skip-list — deferred until a real project needs
  it.
- **Serialized-key shape change** — pre-change snapshots never match a
  `Tree` key. One reindex per stale snapshot; no migration.

## Open Questions

- `staleness.git_aware_skip` is a misnomer once staleness works without
  git. Rename to `staleness.skip_unchanged` (prototype stage — rename in
  place), or leave it and document?
- Should the fixed skip-list be the single source of truth shared with
  the indexer's `DEFAULT_EXCLUDES`, or stay a small independent const in
  `staleness.rs` (`kenn-store` does not depend on `kenn-indexer`)?
