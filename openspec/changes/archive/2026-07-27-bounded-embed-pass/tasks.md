## 1. Chunk the pass

- [x] 1.1 Give `scan_rows` a rowid cursor and a limit:
  `scan_rows(conn, mode, after_rowid, limit)`, ordered by `n.rowid`, with the
  empty-doc-text skip moved into the SQL (`COALESCE(d.doc_text,'') <> ''`) so
  every returned row advances the cursor and an empty chunk means "done".
- [x] 1.2 Loop in `run_embed_pass`: scan → embed → insert → append sidecar, per
  chunk. Accumulate only the vector count and elapsed seconds. Nothing
  corpus-sized may outlive a chunk — in particular `new_entries`, whose
  per-pass accumulation is itself one of the three unbounded allocations.
- [x] 1.3 Thread the chunk size from `EmbeddingsConfig::batch_size` (D3) rather
  than introducing a second constant.
- [x] 1.4 Move the `Full` clear into the **first** chunk's insert transaction
  (D2), so an unavailable embedder — detected on the first submission, before
  any transaction — still cannot wipe vectors.
- [x] 1.5 Keep the per-chunk `vectors.len() != chunk.len()` guard.

## 2. Guards

- [x] 2.1 Bound tested via the observable consequence rather than a fake
  producer: entries are appended per chunk, so a corpus larger than
  `batch_size` yields more than one `seg-` file.
  `the_embed_pass_chunks_its_scan` sets `KENN_EMBED_BATCH_SIZE=2` over the
  5-row corpus and asserts `segments > 1`. Adding that env override was needed
  to make the property testable at all, and it follows the existing
  `KENN_EMBED_URL` / `KENN_EMBED_MODEL` convention — deliberately overriding
  the pass's chunk size and the producer's request cap together, since
  splitting them is the bug.
- [x] 2.2 Test the cursor against interleaved undocumented symbols: every
  documented row embedded exactly once, and the pass terminates.
- [x] 2.3 `a_full_pass_without_an_embedder_keeps_existing_vectors`. Two
  corrections while writing it: a missing model surfaces as
  `Err(Backend("no embedder available"))`, not the `disabled` degradation
  (which is reserved for embedding being switched off), so the assertion is on
  the invariant rather than the failure shape. And the first version counted
  `seg-` files — the wrong artifact: `DELETE FROM vec_knowledge` clears the
  table and leaves every sidecar segment on disk, so the test survived the
  mutation it exists to catch. The distinguishing observable is what a
  *subsequent* incremental pass finds pending: 0 if the rows survived, 5 if
  they were cleared. Mutation (hoisting the clear above the loop) now fails
  `left: 5, right: 0`.
- [ ] 2.4 NOT DONE. A failure on a *later* chunk needs an embedder that fails
  on the Nth call, and `embed_pending`/`reembed` take a concrete
  `&SharedEmbedder` — there is no seam to inject a fake. Testing it means
  making the pass generic over the producer, which is a refactor of the embed
  hot path and out of proportion to the claim. The self-healing property it
  would prove follows from 2.3's mechanism (durable per-chunk inserts + a
  `Pending` filter that selects exactly the rows without vectors), and 2.3 now
  exercises that filter directly.
- [x] 2.5 **Mutation-verified separately.** Single-shot `chunk_size =
  usize::MAX` → the bound test fails with "got 1 — a single segment means the
  whole corpus was embedded and accumulated in one shot". Moving the empty-doc
  skip back out of SQL → the cursor test fails `["alpha"]` vs
  `["alpha","beta"]`, the silent tail-drop D1 predicts. Original wording: Restore the single-shot
  `scan_rows` + one `embed_block_until_ready` and confirm 2.1 fails **on the
  call count**, not on vector contents. Advance the cursor by the last *kept*
  row instead of the last *scanned* row and confirm 2.2 fails. Move the `Full`
  clear before the loop and confirm 2.3 fails. One finding per edit.

## 3. Verify on the real workspace

- [x] 3.1 `kenn index --force` completes and `kenn find` still returns semantic
  hits. Note this does NOT exercise the embed pass: vectors reconcile from the
  committed sidecar by fingerprint, so nothing was pending and no segment was
  written. The embeddable set is **11,043** rows, not the 31,732 first claimed
  (that counted every `knowledge` row).
- [ ] 3.2 NOT DONE, and not claimed: observing ~43 chunks on the real corpus
  needs a genuine full re-embed (~11k embeddings), which a `--force` index does
  not trigger. The chunking itself is covered by 2.1 + 2.5; this task would
  only add scale confirmation.

## 4. Gates

- [x] 4.1 `cargo clippy --workspace --all-targets` — zero warnings.
- [x] 4.2 `just test` green.
- [x] 4.3 `just crap-ci` green. `run_embed_pass` gains a loop; if it crosses the
  threshold, split the per-chunk body into a helper rather than re-baselining.
- [x] 4.4 `cargo fmt --all`, then **clippy once more** (CLAUDE.md §7).
