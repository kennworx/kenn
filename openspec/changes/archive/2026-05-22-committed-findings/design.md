## Context

The findings store (`db/findings/`) is a Lance dataset at `.kenn/findings/`:
one row per `Finding { id, text, embedding, tags, parent_ids, created_at }`.
`store_finding` buffers a finding; `flush` embeds the pending texts, appends
them to the dataset, and rebuilds the BM25 + vector indexes. The dataset is
committed to git and kept merge-clean by content-addressed fragments plus
`reconcile_after_merge`.

`incremental-embedding` shipped the opposite model for code: vectors live in a
committed `fingerprint -> vector` sidecar; the Lance store is derived and
gitignored. The findings store still commits a binary Lance dataset — including
an `embedding` column of raw vectors — which contradicts that model. It is also
opaque: a finding cannot be read, diffed, or reviewed without opening the
dataset.

## Goals / Non-Goals

**Goals:**

- Finding records are committed as human-readable, diff-able, per-finding files.
- The findings Lance store and finding embeddings are derived — rebuilt from the
  records, never committed as binary.
- Finding embeddings reuse the `incremental-embedding` sidecar unchanged.
- Findings behaviour (`store_finding`, `flush`, `search_findings`, supersede /
  tombstone, staleness) is preserved.

**Non-Goals:**

- Changing the finding data model (`id`, `text`, `tags`, `parent_ids`,
  `created_at` are unchanged).
- A new embedding mechanism — the sidecar from `incremental-embedding` is reused
  as-is.
- Migrating an existing committed findings Lance dataset — findings are early
  enough that a one-time re-create is acceptable. A repo that already committed
  the old Lance dataset must `git rm` its `data/` / `manifests/` directories by
  hand; the change does not auto-clean them.

## Decisions

### D1 — A finding record is `.kenn/findings/<id>.json`

Each finding is one JSON file named by its `id` (`fnd_<uuidv4>.json`). The
record holds the authored fields only — `id`, `text`, `tags`, `parent_ids`,
`created_at`. The `embedding` is **not** in the record; it is derived (D3).

Findings are append-only — a correction is a new finding with a `supersedes:`
tag, a deletion a `tombstone:` tag. So a record file, once written, is never
modified. Unique names + write-once ⇒ git never sees a conflict, and a merge is
a plain union of files. `reconcile_after_merge` is therefore deleted.

The in-memory `Finding` keeps its `embedding` field — populated from the sidecar
on rebuild — but the serialized `<id>.json` record omits it; only the authored
fields are written.

### D2 — The findings Lance store is derived, under `.kenn/local/`

The Lance dataset moves from `.kenn/findings/` (committed) to
`.kenn/local/findings/` and is rebuilt from the JSON records. The `local/`
placement is deliberate: `local/` is already gitignored, so the derived store
needs no new ignore rule. (Code's derived store sits at `.kenn/knowledge/` for
historical reasons — `incremental-embedding` minimized churn — but `local/` is
the cleaner home.)

`FindingsStore::open` reads every `.kenn/findings/*.json` and reconciles
embeddings (D3). The rebuild is **not in place**: it writes the new dataset to
a temp directory and atomic-renames it over `.kenn/local/findings/` — the same
temp-dir + swap the code store uses. In-place would be neither crash-safe (a
half-written dataset survives a crash) nor reader-safe (a concurrent finding
query during the rewrite would hit a missing store); the atomic swap makes a
reader always see a complete store — old or new — and a crashed rebuild just
leaves a temp dir the next run sweeps.

The committed `.kenn/findings/` directory then holds only `<id>.json` records
and the `vectors/` sidecar (D3) — no `data/`, `manifests/`, or index segments.

### D3 — A separate findings sidecar, reusing the format

Finding embeddings use the `incremental-embedding` sidecar *format* — the
`KVS1` segments, int8 quantization, `manifest.toml`, compaction — but in their
**own directory**, `.kenn/findings/vectors/`, *not* the code sidecar at
`.kenn/vectors/`. A separate sidecar is required for correctness, not style:

- **Compaction would GC across stores.** `incremental-embedding`'s `compact`
  keeps only entries whose fingerprint is in the *live set*, and the code embed
  job's live set is code fingerprints only. A shared sidecar would have code
  compaction delete every finding vector as "dead" — and a findings compaction
  delete every code vector. Independent sidecars compact against independent
  live sets.
- **One manifest cannot stamp two recipes.** `manifest.toml` records a single
  `[fingerprint].text` recipe. Code's embeddable text is `sig\ndoc`
  (`sig-lf-doc/v1`); a finding's is its raw `text` (`finding-text/v1`). Two
  recipes need two manifests.

`sidecar.rs` is already directory-parameterized (`load_vectors(dir)`,
`compact(dir, live, dim)`, `Manifest::read(dir)`), so the findings sidecar is
largely the existing module pointed at a new directory. One change is needed:
the `embeddable_text` recipe is currently a hardcoded constant (`sig-lf-doc/v1`)
inside `Manifest::current` and the `load_reuse_map` gate. Findings use a
different recipe — a finding's raw `text` — so that recipe becomes a
*parameter*: code passes `sig-lf-doc/v1`, findings pass `finding-text/v1`. No
segment-format change; only the recipe stops being a constant. (The recipe
const exists in `incremental-embedding` today but nothing varies it — findings
are the first second consumer to expose it.)

A finding's key is `sidecar::fingerprint(finding.text)`. The **rebuild** is the
embed point — not `flush` specifically. On rebuild, each finding's vector is
reconciled from `.kenn/findings/vectors/` by fingerprint; **every miss is
embedded then** — pending findings *and* already-committed findings whose
vector is absent (e.g. flushed earlier with no model). The new vectors are
appended as one segment. Findings are a handful of records, not 76k symbols, so
this is synchronous — the decoupled background job that code needs buys nothing
here. The findings Lance store's `embedding` column is populated from the
sidecar at rebuild, never committed.

Embedding "only the pending buffer" would be wrong: a committed finding with a
sidecar miss is never pending again, so it would stay permanently unembedded.
The cache key is the sidecar fingerprint — anything not in the sidecar gets
embedded, regardless of how it got there.

Cross-change seam: a model swap must re-embed *both* sidecars. `kenn update`
(`embedding-model-update`) regenerates the code sidecar; once this change lands
it must also regenerate `.kenn/findings/vectors/` under the new model and
rewrite its manifest. This change owns the findings sidecar's everyday path;
the model-swap path stays with `embedding-model-update`, which must be made
aware of the second sidecar when it is implemented.

### D4 — `flush` writes records first, then derives

`flush` (1) writes each pending finding to its `<id>.json` record, (2) embeds
the new texts and appends a sidecar segment, and (3) rebuilds the derived Lance
store (D2). Step (1) is the commit point: once the records are durably on disk
the next `open` re-derives everything else from them.

Each record write is **atomic** — serialize to `<id>.json.tmp`, then rename to
`<id>.json` — so a crash mid-`flush` never leaves a truncated JSON file that
would break the next `open`. The record files and the `.kenn/findings/`
directory are fsync'd before step (1) is considered complete, so "a crash after
(1) loses no authored data" is a real guarantee, not page-cache optimism. As
defense-in-depth, `open` skips-and-warns on an unparseable record rather than
failing the whole store.

### D5 — A staleness gate skips the rebuild when records are unchanged

Rebuilding the derived Lance store on every `FindingsStore::open` is wasteful —
most opens follow no change. The rebuild writes a stamp,
`.kenn/local/findings/.build-stamp`, recording the record set it built from:
the `.kenn/findings/*.json` **count**, the **newest record mtime**, and an
**`embed_complete`** flag (true when every finding received a vector).

`open` does one `read_dir` of `.kenn/findings/` and compares. It reuses the
existing derived store untouched iff the count and newest mtime match the stamp
*and* `embed_complete` is true; otherwise it rebuilds.

The stamp lives *inside* the derived-store directory, so the atomic swap (D2)
commits the dataset and its `.build-stamp` together — there is no separate
"write the stamp last" ordering to get right.

mtime alone is insufficient. Findings are append-only, so a *new* record always
bumps the newest mtime — but a `git checkout` to a commit with *fewer* findings
deletes record files without touching the rest, leaving the newest mtime
unchanged. The record **count** closes that hole; count + newest-mtime together
characterize the set (every record file is uuid-named and write-once).

`embed_complete` lets the gate coexist with the embed-miss recovery (D3): a
rebuild that ran with no model leaves vectors null and records
`embed_complete = false`, so the next `open` rebuilds and re-attempts embedding
rather than trusting a store with permanent nulls.

### D6 — One writer: the rebuild is serialized by an advisory lock

Two processes can both `open` the findings store at once — the MCP server and a
CLI command, say — and both find the staleness gate (D5) fired. Two concurrent
rebuilds would race on the `.kenn/local/findings/` dataset, the
`.kenn/findings/vectors/` segments, and `.build-stamp`.

The rebuild therefore runs under an exclusive advisory `flock` on
`.kenn/local/findings.lock` — the one-writer pattern `LanceStore` already uses
for the code store. The lock file is a **sibling** of the derived store
directory, *not* a file inside it: `.kenn/local/findings/` is replaced wholesale
by the atomic swap (D2), so a lock file inside it would change identity on every
rebuild and stop excluding anything. A sibling at `.kenn/local/findings.lock`
keeps a stable inode across swaps. The sequence:

1. `open` runs the cheap staleness check (D5) — **no lock**. Fresh → reuse.
2. Stale → acquire the rebuild lock (blocking; rebuilds are fast).
3. **Re-check staleness under the lock** — another process may have rebuilt
   while this one waited. Now fresh → release, reuse.
4. Otherwise rebuild + embed misses + write `.build-stamp`, then release.

The double-check in (3) makes the contended case cheap: only the first waiter
rebuilds; the rest fall through to reuse. `flush` takes the same lock for its
rebuild. The lock file lives under the gitignored `.kenn/local/`. The findings
lock is independent of the code store's lock — the two write disjoint
directories, so they never contend.

## Risks / Trade-offs

- **Rebuild cost on open.** Every `FindingsStore::open` rebuilds the Lance store
  from the records. Findings are few (authored notes, not 76k symbols), so this
  is cheap; embeddings are reconciled from the sidecar, not recomputed.
- **Many small files.** One JSON file per finding. Findings accumulate slowly
  and the files are tiny; no compaction is needed (unlike the vector segments).
- **Removing `reconcile_after_merge`.** The binary-union merge healing is
  deleted. Safe — it existed only because the Lance dataset was committed;
  derived stores never need it, and the JSON records merge as plain files.
- **Loss of an existing committed findings dataset.** A repo that already
  committed a `.kenn/findings/` Lance dataset will not auto-migrate. Acceptable
  per Non-Goals — findings adoption is early; re-authoring or a manual export is
  the escape hatch.
