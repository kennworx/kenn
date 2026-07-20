## 1. Vector sidecar format and I/O

- [x] 1.1 Define the segment file format — header (`magic`, `dim`, `quant`, count) followed by entries sorted by fingerprint: `u64 fingerprint`, `f32 scale`, `dim × i8` (design D1).
- [x] 1.2 int8 quantize / dequantize — per-vector symmetric scalar quantization, `scale = maxabs / 127` (design D2).
- [x] 1.3 Write a segment — given `(fingerprint, vector)` pairs, emit `seg-<sha>.bin` sorted by fingerprint.
- [x] 1.4 Read + union — load all segments plus `baseline.bin` into one `fingerprint → vector` lookup.
- [x] 1.5 `manifest.toml` read/write — model identity, `dim`, `quant` (design D4).

## 2. Fingerprint reconciliation at index time

- [x] 2.1 Compute the `xxh3-64` fingerprint of each symbol's `embeddable_text`; pin the text normalization so formatting noise cannot churn fingerprints.
- [x] 2.2 `kenn index` joins sidecar vectors into the `embedding` column by fingerprint; misses are left null.
- [x] 2.3 Gate reconciliation on the manifest — a model-identity mismatch yields an empty reuse map (design D4).
- [x] 2.4 Test: an index run reuses every committed vector whose fingerprint is unchanged and leaves only the diff null.

## 3. Background embedding job

- [x] 3.1 The job scans the structural store for null-embedding rows, embeds them, int8-quantizes, and appends one segment.
- [x] 3.2 Hot-swap the new vectors into the searchable Lance store and rebuild the IVF_PQ index.
- [x] 3.3 Wire the job into the MCP cold-start orchestration — BM25 and already-cached vectors serve immediately; coverage fills in as the job runs (design D5).
- [x] 3.4 Add a CLI trigger that runs the same job headless (CI / scripted use).
- [x] 3.5 Degrade cleanly when no model is available — cached vectors still serve, misses stay null, the situation is reported.
- [x] 3.6 Test: after an index leaving `M` misses, the job embeds exactly `M` symbols and appends an `M`-entry segment.

## 4. Segment compaction

- [x] 4.1 Compaction — k-way merge of all segments + baseline, retain `fp ∈ live ∧ model == manifest`, write one `baseline.bin`, remove the superseded segments (design D3).
- [x] 4.2 Throttle compaction — run only every K segments or above a size threshold; a build reads an un-compacted log directly.
- [x] 4.3 Test: compaction drops dead fingerprints and stale-model entries; an un-compacted multi-segment log still reconciles correctly.

## 5. git layout and integration

- [x] 5.1 Create the `.kenn/vectors/` layout and route the sidecar there — in the source repo, not under `store_root`.
- [x] 5.2 Invert `.gitignore` — gitignore `.kenn/knowledge/` (now derived), track `.kenn/vectors/`.
- [x] 5.3 The job writes segments into the tracked `.kenn/vectors/` path so they ride the task's normal commit; document an optional `.gitattributes` LFS opt-in for `.kenn/vectors/**` (design D7).

## 6. Verification

- [x] 6.1 `cargo clippy --workspace --all-targets` to zero warnings.
- [x] 6.2 Test: a fresh worktree rebuilds derived state and reuses committed vectors, embedding only its own diff.
