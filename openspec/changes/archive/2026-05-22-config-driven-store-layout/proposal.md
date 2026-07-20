## Why

The kenn store's on-disk layout is hardcoded and scattered. `Store::open`
hardcodes the `.kenn` directory name; `layout.rs` hardcodes `local/`, `live/`,
and the snapshots path; `findings/store.rs` hardcodes `findings/` and
`findings/vectors/`; the indexer driver writes `scip-{slug}.scip` straight into
`.kenn/`. `roots::resolve` reads a few `[workspace]` keys, but `index_workspace`
ignores the split and co-locates source and store anyway.

The consequences are concrete: derived intermediates (the ~5 MB `scip-*.scip`)
land next to committed data and escape `.kenn/.gitignore`; the derived store
cannot be relocated; and there is no way to share a derived index across the
branches or worktrees of one repository — every branch switch forces a reindex.

## What Changes

- A single `Layout` type, resolved once from config, becomes the **sole source
  of every store path**. No crate joins `.kenn` / `local/` / `findings/` etc.
  on its own.
- Config gains **one relocatable root** — the derived-store root. The committed
  root is always `<source_root>/.kenn`: resolved through `Layout`, never
  hardcoded, but deliberately *not* a free config knob — a settable committed
  root could point version-controlled embeddings out of the repo.
- The derived-store root MAY point **outside the repo** — e.g. a global
  per-project cache (an XDG path) shared across all branches and worktrees.
  Snapshot resolution becomes keyed by the staleness key, and retention is
  LRU-bounded, so a branch switch picks a matching prebuilt snapshot — for any
  branch whose snapshot is still warm — instead of reindexing.
- The **committed vs. derived** split becomes explicit. All derived
  intermediates — including `scip-*.scip` — resolve under the derived-store
  root, fixing the git-stage leak.
- `index_workspace`, `Store::open`, the findings store, and the indexer driver
  take the resolved `Layout` instead of a single conflated `workspace_root`.

## Capabilities

### New Capabilities

- `store-layout`: the config-driven, centrally-resolved on-disk layout — the
  resolved-but-fixed committed root, the relocatable (optionally global) derived
  root, the committed-vs-derived classification of every store artifact,
  staleness-keyed snapshot resolution with LRU retention, and the rule that
  every component resolves paths through it.

### Modified Capabilities

- `index-store-db`: the code-graph store's location stops being the hardcoded
  `.kenn/local/` and becomes the configured derived-store root (default
  `.kenn/local/`).
- `mcp-orchestrated-indexing`: the startup snapshot-freshness decision stops
  checking only the single `live` snapshot and instead scans the retained
  snapshot set for a staleness-key match — the `decide_startup_state` change a
  shared derived root requires.

## Impact

- `kenn-store` — `layout.rs` (the new `Layout`), `roots.rs` (subsumed/extended),
  `Store::open`, `findings/store.rs`, `.kenn/.gitignore` generation.
- `kenn-indexer` — `index_workspace` and `driver.rs` take `Layout`; `scip-*.scip`
  is written under the derived-store root, not next to committed data.
- `kenn-cli` / `kenn-mcp` — thread the resolved `Layout` through; this also
  resolves the latent `store_root`-vs-`source_root` confusion in `serve_stdio`.
- `kenn-config` — a new `[layout]` section with a single `derived_root` key.
- Coordinates with `mcp-background-reindex` — that change's reader registry,
  `index.lock` use, and `decide_startup_state` rewrite all sit in the derived
  store this change relocates; see design.md.
- New config keys only; defaults preserve today's in-repo layout — **no
  migration for existing repos**. A global derived root is strictly opt-in.
