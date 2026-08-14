## Why

`incremental-embedding` promises the embed pass costs bounded memory:

> Texts and vectors SHALL NOT be accumulated for the whole corpus before
> submission… Peak memory SHALL be bounded by one scan batch plus one in-flight
> producer request, **independent of corpus size**.

`run_embed_pass` does the opposite. `scan_rows` returns a `Vec<Unembedded>` for
the entire match set with no `LIMIT`, every text is collected, and
`embed_block_until_ready` returns `Vec<Vec<f32>>` — all vectors at once — before
the first row is written. Three corpus-sized allocations are live together, and
the vectors dominate: 768 floats × 4 bytes ≈ 3 KB per row.

On this repo the embeddable set — documented name rows, which is what the scan
actually returns — is **11,043 rows ≈ 34 MB**. (An earlier draft of this
proposal said 31,732 ≈ 93 MB; that was every `knowledge` row, including doc
rows and undocumented names the scan filters out. The corrected figure is
smaller and the argument is unchanged: it is linear in the corpus.) A
million-symbol monorepo is still multiple GB resident before a single vector is
persisted.

The guarantee did not survive the `replace-lance-with-sqlite` migration: the
Lance implementation consumed a scan stream one `RecordBatch` at a time, and the
rewrite replaced it with a single `SELECT` into a `Vec`.

Two things hid it. `EmbedMode::Pending` — the common path — only selects rows
missing a vector, so an incremental run after a small edit loads a handful.
Only `EmbedMode::Full` (fresh clone, model change, `--force`) is unbounded, and
those are the runs nobody watches. And the layer below *is* batched:
`remote.rs` chunks by `EmbeddingsConfig::batch_size` (default 256), which bounds
each HTTP request while doing nothing about the caller's `Vec`. The local
`llama` backend does not chunk at all. The system looks batched from inside the
embedder while the pass above holds everything.

## What Changes

- **Chunk the pass by the same `batch_size` the embedder already uses**, so the
  two layers agree instead of one silently defeating the other. Each chunk is
  scanned, embedded, inserted, and appended to the sidecar before the next is
  pulled; only counts and elapsed time accumulate across chunks.
- **Scan by rowid cursor** rather than re-querying, so `Full` (which has no
  "already embedded" filter) terminates and `Pending` does not depend on its
  own writes to advance.
- **Push the empty-text skip into SQL.** Undocumented symbols are filtered by
  the query rather than dropped after the fact, so a chunk boundary cannot land
  mid-skip and every returned row advances the cursor.
- **BREAKING (failure semantics), deliberately:** a `Full` pass that fails
  midway now leaves the chunks it completed instead of rolling back to the
  previous vectors. See design D2 — this is a net improvement, but it is a
  behavior change and is called out rather than absorbed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `incremental-embedding`: the streaming requirement keeps its guarantee and
  loses its Lance vocabulary (`RecordBatch`, `try_collect`, "row groups"), and
  gains the failure-semantics clause the chunking introduces.

## Impact

- `crates/kenn-store/src/db/jobs.rs` — `run_embed_pass`, `scan_rows`,
  `insert_vectors`.
- No schema change, no API change, no reindex required. A pass that ran before
  runs the same, in bounded memory.
- Split out of `retire-lance-vocabulary`, whose Non-Goals rule out code changes.
  That change reworded the surrounding requirements; this one restores the
  behavior one of them describes.
