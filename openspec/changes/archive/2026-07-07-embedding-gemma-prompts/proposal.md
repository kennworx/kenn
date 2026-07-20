## Why

> **Status: MEASURED — reshaped.** The A/B eval ran (isolated vector arm, 200
> self-supervised doc→symbol queries over a 7,363-symbol corpus,
> `crates/kenn-store/examples/prompt_ab.rs`):
> | arm | r@1 | r@10 | mrr |
> |---|---|---|---|
> | none (baseline) | 0.660 | 0.950 | 0.790 |
> | query-only | **0.695** | 0.955 | **0.808** |
> | query+doc | 0.680 | 0.960 | 0.802 |
>
> Verdict: the win is **small and driven by the QUERY prompt** (+0.035 r@1,
> +0.018 mrr, free — no re-embed). The **document** prompt adds ~nothing over
> query-only and slightly *hurts* r@1 — so the expensive half (corpus re-embed +
> recipe bump) is **not justified by this eval**. Caveats: single N=200 run (no
> variance estimate); r@10 is near a ceiling (0.95) from self-supervised leakage,
> so trust r@1/mrr; InstructionRetrieval (code-specific) prompt and the
> fused-hybrid no-regression check were not run. **Recommendation: adopt
> query-only prompting (cheap, no migration); defer the document prompt pending a
> stronger eval.** See Design → D5.

kenn embeds with `EmbeddingGemma-300M`, a model **trained to receive
task-specific instruction prompts** on each side of a retrieval pair. Today kenn
sends **raw text on both sides** — the query and the indexed document are
tokenized identically (BOS + raw text, mean-pool, L2-normalize) with no prompt.
Evidence:

- `crates/kenn-embed/src/llama.rs:145-156` — `str_to_token(text, AddBos::Always)`
  on the raw text; the only prepend is BOS, applied to every input.
- `crates/kenn-embed/src/shared.rs:206-218` — `embed` (corpus) and `embed_query`
  (query) differ **only in scheduler priority** (Low vs High), not prompting.
- Workspace-wide there is no `task:`, `title:`, `text:`, or `search result`
  literal — the prompts are unused on both sides.

EmbeddingGemma's published retrieval scores are measured *with* its prompts, so
kenn is very likely leaving retrieval quality on the table. The document side is
**doc prose** (`COALESCE(d.doc_text,'')`), which is natural language — a good fit
for these retrieval prompts — so a real gain is plausible. But the effect on
code-doc prose specifically is not known; it must be measured, not assumed.

**Coupled invalidation trap (context for the deferred half).** Vectors are
reused by `xxh3_64(embeddable_text)` plus a manifest recipe tag
(`CODE_TEXT_RECIPE = "doc/v1"`, `crates/kenn-store/src/embed/sidecar/manifest.rs:20`).
A **document** prompt would be applied inside the producer at embed time, so
`embeddable_text` bytes would not change and the fingerprint would not move —
adopting it MUST come with a recipe-tag bump or existing indexes silently keep
stale prompt-less vectors. This cost is part of why the measured-as-worthless
document prompt is deferred. The **query-only** scope shipped here has no such
coupling: query vectors are computed fresh per search and never persisted.

## What Changes (query-only scope, per the measured verdict)

- Introduce an explicit **embed kind** (query vs document) at the producer
  boundary. Today the kind is implicit in which method is called
  (`embed_query` = query, `embed` = document); make it explicit so the producer
  can apply the right prompt.
- The `EmbeddingGemma` producer (in-process, and any remote producer whose
  configured model id is EmbeddingGemma-family) SHALL prepend the model's query
  task prompt to **query-kind** embeds only:
  - query: `task: search result | query: {q}`
  - document: raw text, unchanged (the document prompt is deferred — measured
    as adding nothing over query-only while costing a full re-embed).
  Non-EmbeddingGemma models SHALL NOT receive the prompt for either kind (it is
  model-specific — a generic remote model would be harmed by it).
- ~~Bump `CODE_TEXT_RECIPE`~~ — not needed: corpus embedding is untouched, no
  re-embed, no migration.
- The **A/B eval** that gated this change is already landed and run
  (`crates/kenn-store/examples/prompt_ab.rs`); the fused-hybrid no-regression
  check it skipped moves to implementation verification.

The stored `embeddable_text` stays clean (prompt applied at embed time only), so
display/debug paths, fingerprints, and every stored vector are unaffected.

## Capabilities

### Modified Capabilities

- `embedding-producer`: the producer boundary gains an explicit query-vs-document
  embed kind, and the EmbeddingGemma producer applies the model's query task
  prompt to query-kind embeds (documents stay raw; doc prompt deferred).

## Impact

- **Behavior:** free-text queries are embedded with the query prompt at search
  time; corpus vectors are byte-identical to today. Measured effect: r@1
  0.660 → 0.695, MRR 0.790 → 0.808 on the isolated vector arm.
- **Compatibility:** none — no re-embed, no recipe bump, existing indexes work
  unchanged the moment the binary updates.
- **Deferred:** the document prompt (and its recipe bump + full re-embed)
  awaits a stronger eval; see design D3/D5. If `shared-vector-cache` lands
  first, a future bump becomes non-destructive (new generation dir).
- **Related finding (separate):** the invalidation trap above is one instance of
  a broader gap — kenn's re-embed/re-index gates are hand-maintained version
  constants not derived from indexer source. Tracked as a note, not in scope here.
