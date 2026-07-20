## Why

Embedding is the expensive step; the vector for a given `embeddable_text` is
already content-addressed (`fingerprint = xxh3_64(embeddable_text)`,
`crates/kenn-store/src/embed/sidecar/quant.rs:17`), so identical text *could* be
embedded once and reused everywhere. Today it isn't, because the cache is packaged
per-checkout and single-generation:

- A **relative** `[vectors] location` resolves against `source_root`
  (`crates/kenn-store/src/layout/resolve.rs:38-45`), so each worktree gets its own
  dir — worktrees can't share a repo-local vector dir.
- `"global"` keys the dir by `xxh3_64(canonical source_root)` (`resolve.rs:100`),
  so worktrees/clones at different paths **fragment** rather than share.
- A dir holds **one** `(model, dim, quant, recipe)`; a recipe/model bump calls
  `reset_vectors`, which **wipes the whole dir**. A shared dir is therefore unsafe
  the moment two checkouts are on different generations (e.g. mid-migration, or
  the `embedding-gemma-prompts` recipe bump).

The graph snapshot must stay per-worktree (isolated writes; the existing
`open_for_read` parent-fallback and the "writes never touch parent" rule,
`worktree.rs`), but the **vector cache is the one store safe to share-write** —
content-addressed appends are idempotent + atomic (`io.rs:47-113`). The only
unsafe operation is the destructive `reset`. Remove it (via per-generation
subdirs) and a shared vector cache becomes correct.

## What Changes (three sequenced phases)

**Phase 1 — relative location resolves at the git root.** A relative
`[vectors] location` SHALL resolve against the **main worktree** (git root), not
the per-worktree `source_root`, so `location = "vectors"` puts every worktree's
vectors in one repo-local dir. Reuse the existing `resolve_main_worktree`
(`worktree.rs:35`, `git worktree list --porcelain`); fall back to `source_root`
when not in a git tree. Independent, small, useful on its own.

**Phase 2 — multi-generation store + GC.** Namespace vectors by generation —
`<vectors_root>/<model>/<dim>/<quant>/<recipe>/…` — so multiple generations
coexist as sibling dirs. A recipe/model change writes a **new** generation dir and
leaves old ones intact; **`reset_vectors` is deleted** (nothing to wipe). Add
garbage collection: evict least-recently-used / over-cap generations, tracked by
access time, with a lock scoped to GC only (appends stay lock-free).

**Phase 3 — default to the shared repo dir (gated on Phase 2).** Once generations
coexist safely, change the **default** vectors location to the git-root-relative
shared subdir (Phase 1), so linked worktrees reuse each other's vectors **out of
the box** with no config. Concurrent multi-worktree writes are safe because they
are content-addressed appends into per-generation dirs.

## Capabilities

### Modified Capabilities

- `store-layout`: relative vectors location resolves at the git root; the default
  vectors location is a git-root-shared subdir (Phase 3).
- `incremental-embedding`: vectors are stored per generation (no destructive
  whole-dir reset) and the cache is garbage-collected.

## Impact

- **Behavior:** worktrees/clones on the same generation embed identical text once
  and reuse it; a recipe/model bump no longer wipes other checkouts' vectors.
- **Sequencing:** Phase 3 MUST follow Phase 2 — defaulting to a shared dir while
  `reset` still wipes would let one worktree's migration destroy every worktree's
  cache.
- **Open decision (Phase 3):** the current default `<committed_root>/vectors` is
  git-tracked, which is what lets a fresh clone search offline (embedding-producer
  spec, "a fresh clone searches without a model"). A git-root-shared subdir SHOULD
  preserve this by resolving to the **main worktree's committed** vectors dir
  (linked worktrees write content-addressed segments the main worktree can commit)
  — versus a gitignored local-only cache that drops clone-portability. Decide
  before Phase 3; default toward preserving the committed-sidecar property.
- **Note:** cross-*machine* sharing is the same design with `vectors_root` on a
  shared mount — no further mechanism, out of scope here.
