## Context

`kenn index` builds the structural store (redb + a Lance dataset of symbols,
signatures, docs, BM25 indexes) from source — fast, even on a large repo. The
embedding pass is decoupled and currently re-embeds the whole committed corpus
wholesale via a temp-store + atomic swap.

Two problems remain. (1) The embedding pass is O(corpus), not O(diff): a
one-file change still costs ~169 s on a 76k-symbol repo. (2) Embeddings — the
output of an expensive external model — are not persisted anywhere shareable, so
every worktree and clone re-embeds from zero, and vector search is degraded
until the pass finishes.

An embedding is a pure deterministic function of `(embeddable_text, model)`.
The `embeddable_text` of a symbol is its signature joined with its doc comment
(`sig\ndoc`, or `sig` alone). An `xxh3-64` fingerprint of that text is therefore
a stable content-addressed key for a vector.

## Goals / Non-Goals

**Goals:**

- Embeddings become an incremental artifact: the background job embeds only
  symbols whose fingerprint is not already cached — O(diff), not O(corpus).
- Embeddings become a git-persisted artifact: committed once, reused by every
  worktree and clone, which then embed only their own diff.
- The committed footprint stays small enough for ordinary git (LFS optional).
- A worktree is BM25-searchable immediately and vector-searchable progressively;
  no fully-degraded window.

**Non-Goals:**

- Model-version swaps. A change of model invalidates every vector; that is a
  synchronous full rebuild owned by `kenn update` / `embedding-model-update`,
  not this change.
- Committing findings records. Findings decompose identically (records as
  committed files, finding embeddings via this same sidecar) but ship as a
  separate change.
- Replacing Lance's own index-time quantization. Lance keeps building IVF_PQ
  over a float column; the int8 here is the on-disk *sidecar* representation.

## Decisions

### D1 — Sidecar format: content-addressed `fingerprint → vector`

The committed artifact is a `fingerprint → vector` map, where `fingerprint` is
the `xxh3-64` of the symbol's `embeddable_text`. Keying by content hash (not by
symbol id) buys two properties for free:

- **Dedup is trivial.** The same fingerprint in two segments carries a
  byte-identical vector (same text, same model) — merge is a set union, no
  "newest wins", no ordering.
- **GC has a clear rule.** An entry is dead iff its fingerprint is absent from
  the current structural build's live set.

A segment is a flat binary file: a header (`magic`, `dim`, `quant`, count),
then entries sorted by fingerprint — `u64 fingerprint`, `f32 scale`, `dim ×
i8`. Sorted order makes the merge a linear k-way pass.

### D2 — int8 / 768-d, full dimension, no Matryoshka truncation

A measured sweep (EmbeddingGemma-300M, code corpus, item-to-item retrieval
against the full-precision ranking — see `tmp/embed-bench/mrl-RESULTS.md`):

| representation | R@10 vs 768·f32 | bytes/vec | 76k baseline |
|---|--:|--:|--:|
| 768 · f32 (today) | 1.000 | 3072 | 223 MB |
| **768 · int8** | **0.994** | **772** | **56 MB** |
| 512 · f32 (MRL) | 0.850 | 2048 | 149 MB |
| 256 · f32 (MRL) | 0.726 | 1024 | 74 MB |

int8 scalar quantization (per-vector symmetric scale, `q = round(x/scale)`,
`scale = maxabs/127`) is near-lossless — and **dominates** Matryoshka
truncation: `768·int8` is both smaller *and* higher-fidelity than `512·f32` or
`256·f32`. Dimension truncation reshuffles 15–27 % of the top-10 for a smaller
saving. So: keep the full 768 dimensions, quantize to int8. (768 is the MRL
prefix superset, so a deployment may still truncate at *load* time for a leaner
in-memory index — but the committed artifact stays 768.)

### D3 — Append-log of segments, compacted into a baseline

Each background-job run appends one **segment**, `seg-<sha>.bin`, containing
only the fingerprints it embedded. Uniquely named ⇒ git never sees a conflict
(new file, never a modified one) — concurrent branches each append cleanly.

**Compaction** runs at task start, throttled to every K segments (or a size
threshold):

```
live   = { fingerprint of every row in the freshly-built structural store }
merged = ⋃ read(every committed segment + baseline)      # k-way merge, sorted
kept   = { (fp,vec) ∈ merged : fp ∈ live AND model == active_model }
write  kept → baseline.bin   (one file)
git rm the old segments ; git add baseline.bin
```

One linear pass, O(total entries). `fp ∈ live` evicts deleted/edited symbols;
`model == active_model` evicts stale-model vectors. A build does not *need* to
compact to use the log — it unions all segments in memory for the lookup;
compaction only bounds file count and git size.

### D4 — Manifest: the vector generation stamp

`.kenn/vectors/manifest.toml` records *only* fields that, if changed, invalidate
every committed vector — nothing that churns per-append, so it rarely conflicts
in git:

```toml
# .kenn/vectors/manifest.toml — the vector "generation" stamp.
# Rewritten only on a generation change (kenn update / model swap),
# never on a routine incremental append.

format_version = 1                  # segment + manifest file-format version

[model]
name      = "embeddinggemma-300M"   # human-readable, for logs
gguf_xxh3 = "9f3a1c2e7b4d8a06"      # xxh3-64 of the GGUF weights — authoritative identity
prompt    = "none"                  # task/instruction prefix applied before embedding

[vector]
dim   = 768                         # full model dimension (no MRL truncation)
quant = "int8-sym-pervec"           # per-vector symmetric scalar int8 + f32 scale
norm  = "l2"                        # vectors are L2-normalized

[fingerprint]
hash = "xxh3-64"                    # key = hash of embeddable_text
text = "sig-lf-doc/v1"              # the embeddable_text recipe + normalization version
```

Each field is something that silently corrupts the sidecar if it drifts:

- **`[model]`** — `gguf_xxh3` is the authoritative identity (catches a
  regenerated GGUF with an unchanged filename); `prompt` matters because
  EmbeddingGemma is prompt-sensitive — a changed task prefix changes every
  vector; `name` is for humans.
- **`[vector]`** — `dim` / `quant` / `norm` define how stored bytes decode into
  a vector.
- **`[fingerprint]`** — `hash` + `text` define how *keys* are derived; if the
  `embeddable_text` normalization recipe changes, every key shifts and old
  entries become unreachable, so `text` carries a versioned tag.

A worktree computes its own stamp from its active embedder and compares. Any
field differs ⇒ the committed sidecar is unusable as-is ⇒ reconciliation treats
it as a full miss ⇒ `kenn update` must regenerate it. The manifest is one atomic
generation gate — the boundary `embedding-model-update` keys off.

The manifest is created when the sidecar is first established and rewritten
*only* by `kenn update`. Incremental appends read it — to confirm they embed
under the same generation — but never modify it, which is what keeps it
conflict-free.

### D5 — Background embedding job in the MCP cold-start lifecycle

`kenn index` produces the structural store and a fingerprint reconciliation
join — every cached fingerprint's vector is filled in immediately; misses are
left null. The MCP server then runs the **embedding job** as a background task
in its existing cold-start orchestration: BM25 search is live at once, vector
coverage fills in as the job embeds the misses and hot-swaps them in. A CLI
trigger runs the same job headless (for CI / scripted use).

### D6 — git layout; the committed/derived line

```
<repo>/.kenn/
  vectors/                 COMMITTED  (git, optionally LFS)
    manifest.toml
    baseline.bin
    seg-<sha>.bin …
  local/ (or store_root)   GITIGNORED, rebuilt per worktree
    redb/ … · knowledge/ (Lance + BM25 + IVF_PQ) · building/ · snapshots/ …
```

`.gitignore` inverts: the current comment declaring `.kenn/knowledge/` committed
is replaced — `knowledge/` is derived and gitignored; `.kenn/vectors/` is
tracked. The sidecar must live inside the source repo (it is git); `store_root`
keeps redirecting only the *local*, derived store.

### D7 — LFS is optional

At 56 MB baseline + KB-to-MB delta segments, the sidecar fits ordinary git.
A repo may opt into LFS for `.kenn/vectors/**` via `.gitattributes`; kenn does
not require it. LFS relocates the binary out of `.git` history but adds no delta
compression — the append-log + compaction is what actually bounds growth.

## Risks / Trade-offs

- **Fingerprint churn.** If `embeddable_text` is not normalized
  deterministically, formatting noise perturbs fingerprints and causes false
  cache misses (needless re-embedding). The normalization of `sig\ndoc` must be
  pinned and stable — this is the load-bearing assumption.
- **Committing generated data.** Vectors in git is unusual. Mitigations: the
  artifact is small (int8), content-addressed (dedups across history),
  append-only (conflict-free), and compacted (bounded). It is the *one*
  expensive non-source-derivable artifact; everything else stays derived.
- **git history growth.** git never forgets blobs; compaction shrinks the
  working set, not history. Acceptable for an int8 sidecar; LFS is the escape
  hatch if a long-lived repo's history bloats.
- **Stale int8 sweep corpus.** The D2 numbers are from a 2000-text code corpus;
  the int8 ≈ lossless conclusion is corpus-robust, but a labeled recall set on
  the real corpus would harden the quantization choice. Low risk — int8 loss is
  consistently negligible across every dimension measured.
- **Self-reference metric.** D2's retrieval numbers are agreement-with-768, not
  labeled absolute quality. This affects only the (rejected) truncation option;
  the int8 decision is measured identically to its f32 baseline and holds.
