## Why

The embedding pass re-embeds the **entire** committed corpus on every run —
~169 s for a 76k-symbol repository — even when a task touched only a handful of
files. And embeddings are the one expensive, externally-produced artifact (the
embedding model), yet nothing persists them: every git worktree and every fresh
clone must re-embed the whole corpus from zero.

Embeddings are a pure, deterministic function of `(embeddable_text, model)`.
That makes them cacheable and shareable. This change turns them into an
incremental, git-persisted artifact: `kenn index` stays fast and
structural-only; a background job embeds only the **changed** symbols; the
resulting vectors are committed to git so every other worktree and clone reuses
them and pays the model only for its own diff.

## What Changes

- A new committed **vector sidecar** — a `fingerprint → vector` map (int8,
  768-d), stored as an append-log of uniquely-named segments compacted into a
  baseline. The only new artifact added to git.
- `kenn index` **reconciles** against the sidecar: every symbol whose `sig+doc`
  fingerprint is already present gets its committed vector joined in; only
  misses are left unembedded.
- A **background embedding job** — part of the MCP server's cold-start
  orchestration, and runnable from the CLI — embeds the misses and appends a new
  sidecar segment. MCP serves BM25 immediately and gains vector coverage as the
  job completes.
- **Compaction** folds segments + baseline into a single baseline, dropping
  dead entries (fingerprint absent from the live corpus) and stale-model
  entries.
- Vectors are stored **int8-quantized** (per-vector scalar) — a measured
  near-lossless 4× shrink (see design D2).
- The committed/derived line is redrawn: `.kenn/vectors/` is committed; the
  `knowledge/` Lance store, BM25 and IVF_PQ indexes are reclassified as derived
  and gitignored — they rebuild per worktree.

`kenn update` — the synchronous full re-embed for a model-version swap — is
unchanged; it remains the rare model-swap path covered by `embedding-model-update`.

## Capabilities

### New Capabilities
- `incremental-embedding`: the committed vector sidecar, fingerprint-based
  reconciliation, the background embedding job, and segment compaction —
  embeddings as an incremental, git-persisted, per-diff artifact.

### Modified Capabilities

<!-- None: the embedding pass behavior introduced here is captured fully in the
     new capability; no already-synced spec changes its requirements. -->

## Impact

- **New committed artifact:** `.kenn/vectors/` — segments, `baseline.bin`, and a
  `manifest.toml` (model identity, dim, quant). Optionally git-LFS-tracked.
- **`.gitignore` inverts:** `.kenn/knowledge/` becomes gitignored (derived,
  rebuilt per worktree); `.kenn/vectors/` becomes tracked.
- **`kenn index`:** gains a fingerprint reconciliation join; structural output
  unchanged.
- **Embedding pass:** becomes a background job (MCP cold-start + a CLI trigger)
  that embeds only the diff, instead of a wholesale re-embed.
- **MCP server:** cold-start orchestration runs the background embedding job;
  vector search lights up progressively rather than being degraded until a
  separate pass finishes.
- **Storage:** committed vectors ~56 MB at 76k symbols (int8/768); delta
  segments KB-to-MB.
- **Out of scope — consistent follow-up:** findings decompose the same way
  (finding records committed as per-finding files, finding embeddings via the
  same sidecar). A separate change.
