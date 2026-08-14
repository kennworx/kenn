## Context

`run_embed_pass` (`crates/kenn-store/src/db/jobs.rs`) is the one place vectors
are produced. Today:

```rust
let pending = scan_rows(&conn, mode)?;                    // Vec<Unembedded>, no LIMIT
let texts: Vec<&str> = pending.iter().map(…).collect();
let Some(vectors) = embedder.embed_block_until_ready(&texts).await? else { … };
let new_entries = insert_vectors(&conn, &pending, vectors, mode)?;
sidecar::append_vectors(…, &new_entries)?;
```

Every binding is corpus-sized, including `new_entries` — so appending the
sidecar once at the end is itself an accumulation, not just the embed call.

Two existing behaviors constrain the fix:

- **`Full` clears inside the insert transaction.** The comment is explicit:
  "We scan and embed *before* touching `vec0`, so a disabled embedder or an
  embed error never wipes existing vectors (the `Full` clear happens in the
  same transaction as the re-insert)."
- **`Pending` filters on `vec_knowledge`.** `AND n.rowid NOT IN (SELECT rowid
  FROM vec_knowledge)` — so it self-advances as rows are inserted, while `Full`
  has no such filter and would re-return the same rows forever.

## Goals / Non-Goals

**Goals:**

- Peak memory bounded by one chunk, independent of corpus size — the property
  the spec already requires.
- One batch size, shared by the pass and the embedder backends.
- No change to what gets embedded, or to the vectors produced.

**Non-Goals:**

- Tuning `batch_size`, or making it per-backend.
- Parallelising chunks. The embedder is already a shared, scheduled resource;
  concurrency here is a separate question with its own evidence.
- Touching the sidecar format.

## Decisions

### D1 — Rowid cursor, not `OFFSET`, and not re-query

**Decision.** `scan_rows` takes `after_rowid` and `limit`, orders by
`n.rowid`, and returns at most `limit` rows. The loop advances the cursor to
the last returned rowid.

**Why not `OFFSET`.** `OFFSET` re-walks the skipped prefix on every chunk —
quadratic over the corpus, on the exact path that motivated the change.

**Why not rely on `Pending`'s filter to self-advance.** It works for `Pending`
and silently never terminates for `Full`. One mechanism that is correct for
both is worth more than two that are each correct for one.

**Why the empty-text skip moves into SQL.** Rows with empty doc text are
currently dropped *after* the query. With a cursor that is a bug source: a
chunk whose rows are all skipped returns nothing, and a loop that stops on an
empty chunk would stop early with rows remaining. Filtering in SQL
(`COALESCE(d.doc_text,'') <> ''`) makes "empty chunk" mean "done" and every
returned row advance the cursor. Same rows embedded either way.

### D2 — `Full` failure semantics change, and that is an improvement

**Decision.** The `DELETE FROM vec_knowledge` runs in the **first** chunk's
insert transaction. Later chunks insert without clearing.

**Consequence.** A `Full` pass that fails on chunk 3 leaves chunks 1–2 applied
and the pre-existing vectors gone — where today it would have left the old
vectors untouched.

**Why accept it.** The all-or-nothing property was affordable only because
everything was held in memory; it is the same property that costs 3 GB on a
large repo. And the partial state is *self-healing*: `vec_knowledge` rows and
their sidecar entries are durable, so the next `Pending` pass embeds exactly
the rows that are still missing. Today a failed `Full` throws away all
completed work and re-embeds from scratch — strictly worse for the case that
actually hurts, a long full pass on a big corpus.

**Why the clear stays in a chunk transaction rather than moving before the
loop.** If the embedder is unavailable, `embed_block_until_ready` returns
`None` on the *first* chunk and the pass returns `disabled()` before any
transaction opens. Deleting up front would wipe vectors for a workspace whose
embedder simply is not running — the exact failure the original comment guards
against, and the one worth keeping.

### D3 — One batch size, from config

**Decision.** The chunk size is `EmbeddingsConfig::batch_size` (default 256) —
the value `remote.rs` already chunks its HTTP requests by.

**Why this specifically.** The bug's shape is two layers disagreeing: the
embedder batched its requests while its caller handed it the whole corpus. Two
independent constants would leave them free to drift apart again. Sharing one
makes the agreement structural.

**Bound this buys.** 256 rows × 768 × 4 B ≈ **0.8 MB** of vectors in flight,
plus one chunk of text, regardless of corpus size.

## Risks / Trade-offs

- **More sidecar segments.** Appending per chunk writes one `seg-` file per
  chunk instead of one per pass. → That is what `--repack` exists for, and
  accumulating entries to avoid it would reintroduce the corpus-sized
  allocation the change removes.

- **A test that passes without proving the bound.** Asserting "the pass
  produced the right vectors" is true before and after. → The guard must
  observe *chunking*: a fake embedder recording the size of each call, asserting
  more than one call and none larger than `batch_size`. Mutation-verify by
  restoring the single-shot call and confirming it fails on the call count.

- **Off-by-one at the chunk boundary.** A cursor that advances by the last
  *kept* row rather than the last *scanned* row can loop forever. → D1 removes
  the distinction by filtering in SQL; a test covers a corpus with undocumented
  symbols interleaved.

## Open Questions

None. The requirement already specifies the target; this restores it.
