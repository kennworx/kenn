## 1. Finding record files

- [x] 1.1 Define the `.kenn/findings/<id>.json` record format — `id`, `text`, `tags`, `parent_ids`, `created_at` (serde; the `embedding` is *not* in the record).
- [x] 1.2 Write a record atomically — serialize a `Finding`'s authored fields to `<id>.json.tmp`, then rename to `.kenn/findings/<id>.json`; fsync the file and the directory (write-once; the `fnd_<uuid>` id is the filename).
- [x] 1.3 Read records — load every `.kenn/findings/*.json` (direct children only — do not recurse into the `vectors/` sidecar subdir) into `Finding` values (embedding left `None`, filled by reconciliation); skip-and-warn on an unparseable record rather than failing the open.

## 2. Derived findings Lance store

- [x] 2.1 Split path resolution — `findings_dir_for` resolves the committed records directory `.kenn/findings/`; add a resolver for the derived Lance store at `.kenn/local/findings/`.
- [x] 2.2 Relocate the findings Lance dataset to the gitignored `.kenn/local/findings/` (design D2).
- [x] 2.3 `FindingsStore::open` rebuilds the Lance store from the `<id>.json` records — into a temp directory, then atomic-rename over `.kenn/local/findings/` (never in place — crash- and reader-safe, design D2).
- [x] 2.4 Staleness gate (design D5): the rebuild writes `.build-stamp` = `(record count, newest record mtime, embed_complete)` *inside* the rebuilt directory so it rides the atomic swap; `open` does one `read_dir` and reuses the derived store iff count + mtime match and `embed_complete` is true.
- [x] 2.5 Serialize the rebuild under an exclusive advisory `flock` on `.kenn/local/findings.lock` (a sibling of the derived store, stable across the atomic swap), with double-checked staleness — re-check the gate after acquiring the lock so only the first of several racing openers rebuilds (design D6).
- [x] 2.6 Remove `reconcile_after_merge` and the binary-union merge healing — the JSON records are write-once files and git-merge as plain files (design D1).
- [x] 2.7 Test: a fresh `open` reconstructs every finding searchable; the staleness gate skips the rebuild when records are unchanged and rebuilds after a record is added, after one is removed, and after an incomplete prior embed; two concurrent opens rebuild exactly once.

## 3. `flush` writes records first

- [x] 3.1 `flush` writes each pending finding to its `<id>.json` record — the durable commit point — before deriving the Lance store.
- [x] 3.2 Test: a crash-equivalent (drop the store) after records are written but before the Lance store is rebuilt loses no finding — the next `open` re-derives them.

## 4. Finding embeddings via a dedicated sidecar

- [x] 4.1 Parameterize the sidecar's embeddable-text recipe — `sidecar::Manifest::current` and the `load_reuse_map` gate take the recipe as an argument instead of the hardcoded `sig-lf-doc/v1` constant; `incremental-embedding`'s callers pass `sig-lf-doc/v1`, findings pass `finding-text/v1` (design D3).
- [x] 4.2 Point a second sidecar — the `incremental-embedding` `sidecar` module, reused — at `.kenn/findings/vectors/`; a finding's key is `sidecar::fingerprint(finding.text)`, the manifest recipe `finding-text/v1` (design D3).
- [x] 4.3 On rebuild, reconcile each finding's embedding from `.kenn/findings/vectors/` by fingerprint; embed **every miss — pending or already-committed** — and append one segment; compaction runs against the finding live-set only (design D3).
- [x] 4.4 The rebuild (triggered by `open` via the staleness gate, or by `flush`) is the embed point; a rebuild with no model leaves misses null and records `embed_complete = false` for retry. The findings Lance `embedding` column is populated from the sidecar, never committed.
- [x] 4.5 Test: a flushed finding is retrievable by a paraphrase query, and its vector lives in `.kenn/findings/vectors/` — not in any committed Lance column.

## 5. git layout

- [x] 5.1 `.kenn/findings/` is tracked — the `<id>.json` records and the `vectors/` sidecar; the derived `.kenn/local/findings/` Lance store is covered by the existing `local/` ignore rule.
- [x] 5.2 Update the `.kenn/` layout docs (`layout.rs` module doc) for the findings records vs derived store split.

## 6. Verification

- [x] 6.1 `cargo clippy --workspace --all-targets` to zero warnings.
- [x] 6.2 Full findings test suite passes — `store_finding`, `flush`, `search_findings`, supersede / tombstone, staleness, and the merge-clean scenario over the JSON records.
