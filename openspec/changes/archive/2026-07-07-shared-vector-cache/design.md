# Design

## D1 — The key already exists; this is packaging

`fingerprint = xxh3_64(embeddable_text)` is location/path/symbol-id/run
independent (`quant.rs:17`), and segment files self-address by content hash with
idempotent atomic-rename appends (`io.rs:47-113`). Nothing about the *key* changes.
Every phase below only changes *where the bytes live* and *how many generations
coexist*.

## D2 — Why the vector cache (and only it) can be shared-write

kenn's rule is "a worktree's writes never touch the parent" (`worktree.rs` tests),
and the graph snapshot honors it — each worktree builds its own. That rule exists
because the graph/snapshot writer is not concurrency-safe across checkouts. The
vector store is different: writes are **content-addressed, idempotent, and
atomic**, so two worktrees writing the same fingerprint converge on the same file
with no coordination. The cache is therefore the one store we can promote to
shared-write. The sole exception is `reset_vectors` (unlocked, destructive) —
Phase 2 removes it, closing the gap.

## D3 — Phase 1: git-root anchor for relative locations

`resolve_location_spec` (`resolve.rs:28`) currently does
`Some(rel) => source_root.join(rel)`. Change the relative arm to anchor at the
main worktree:

```
main = resolve_main_worktree(source_root)   // worktree.rs:35, porcelain
base = main.unwrap_or(source_root)          // non-git → today's behavior
base.join(rel)
```

All worktrees of one repo resolve `location = "vectors"` to the same
`<main>/vectors`. Absolute and `"global"` arms are unchanged in Phase 1.
`"global"`'s path-keying (the fragmenting one) is superseded by Phase 3's default,
so it needs no change here.

## D4 — Phase 2: generation-namespaced layout, no reset

```
<vectors_root>/<model_id>/<dim>/<quant>/<recipe>/<fp[:2]>/<fingerprint>.<seg>
```

- The generation is the **path**, not a single per-dir manifest gate. Multiple
  generations coexist; a bump writes a new subtree.
- `reset_vectors` (`io.rs:213`, called at `jobs.rs:233`) is **deleted** — a
  recipe/model change never wipes; it populates a new generation dir. This is the
  change that makes sharing safe and also de-fangs the `embedding-gemma-prompts`
  recipe bump (re-prompting no longer nukes the corpus).
- Fold `model_id` into the read-side reuse gate (`load_reuse_map`, `io.rs:259`)
  as defense-in-depth; the path already separates models.

### GC
A keep-everything store grows across generations and (if shared) projects. Add
eviction:
- Track per-generation (and/or per-segment) **last-access** time; touch on reuse.
- A GC pass evicts by LRU until under a configurable size cap. Trigger lazily
  (start of an index run) and/or via an explicit `kenn gc`.
- GC is the **only** operation needing a lock on `vectors_root` (appends stay
  lock-free). Scope the lock to GC.
- A generation that is no current checkout's active `(model,dim,quant,recipe)` is
  the first eviction candidate.

## D5 — Phase 3: shared default, and the committed-sidecar decision

Change the default `vectors_root` from `<committed_root>/vectors`
(`types.rs:52`) to the Phase-1 git-root-relative shared subdir, so linked
worktrees share with zero config.

**Decision to settle first:** today's default is git-tracked, which is what makes
"a fresh clone searches without a model" work (embedding-producer spec). Two ways
to keep worktree-sharing without losing that:

| option | worktree share | clone-portable (offline search) | cost |
|---|---|---|---|
| A. shared subdir = **main worktree's committed** vectors dir | ✅ | ✅ (committed) | linked worktrees write segments into the main tree; GC deletions show in git |
| B. shared subdir = **gitignored local** cache at git root | ✅ | ❌ (nothing committed) | simplest; loses offline-clone search unless a separate committed seed is kept |

> **DECIDED: option A** (implemented). The default anchors at the main
> worktree's committed `.kenn/vectors`; for the main worktree itself (and any
> non-git dir) the resolved path is byte-identical to the previous default, so
> nothing moves for single-checkout users. The pre-generation flat
> `code/`/`findings/` dirs stay readable as a same-generation legacy fallback
> (`load_reuse_map_with_legacy`), so committed packs keep serving fresh clones
> with no migration commit.

Lean **A** (preserves the committed-sidecar property; segments are content-
addressed so committing them from the main worktree is natural). Revisit if the
"vectors in git history accumulate across generations" cost (mitigated by GC) is
unacceptable, in which case B + an explicit `kenn export`-style committed seed.

## D6 — Open questions

- Cross-generation GC interacting with a git-tracked dir (option A): evictions are
  git deletions; is that acceptable churn, or should GC only evict from a local
  overlay and never the committed set? (Leans: GC only touches non-committed
  generations.) → **Implemented as the lean:** GC skips any dir holding a
  `pack-*.bin` file, so the committed set is never evicted; only dev-local
  seg-only generations are.
- Determinism assumption for cross-checkout reuse: same model+quant ⇒ same int8
  bytes for the same text. kenn already relies on this by committing vectors;
  int8 + ANN tolerates FP jitter. State it explicitly as a cache invariant.
