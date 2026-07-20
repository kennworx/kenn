# Design

> **Post-measurement scope note.** The D5 eval ran (2026-07-05); verdict:
> **query-only** prompting ships, the document prompt is **deferred**. Sections
> below are updated to that scope — the query prompt is applied at search time
> only, corpus vectors are untouched, and no invalidation is needed.

## D1 — Where the prompt is applied: inside the producer, keyed on model id

The prompt is a property of *how EmbeddingGemma must be invoked*, not of *what
the content is*. So it is applied at the producer, not at the call sites.

```
call site (jobs.rs / support.rs)          producer (llama.rs / remote.rs)
  passes: text + EmbedKind::{Query,Doc}  ─▶  if model_id is embeddinggemma-family
  (embeddable_text stays prompt-free)        and kind is Query:
                                               text = query_prompt + text
                                             tokenize/encode
```

Consequences:
- `embeddable_text` (stored, fingerprinted, shown in debug) stays clean. Only the
  bytes fed to the tokenizer carry the prompt.
- A remote producer pointed at a **non-EmbeddingGemma** model (ollama, lm-studio)
  does not get Gemma prompts — the decision is `f(model_id, kind)`, implemented
  once and shared by both the in-process and remote producers.
- Only the `Query` kind is prompted; `Document` embedding is byte-for-byte
  unchanged, so stored vectors, fingerprints, and the reuse gate are all
  unaffected (D3).

**Rejected alternative** — prepend at the call site and *store* the prompted text
in `embeddable_text`. That would auto-invalidate the fingerprint (no recipe bump
needed), but it pollutes the stored text, leaks a model-specific detail into a
model-agnostic column, and still needs a separate query-side path. Not worth it.

## D2 — Making the embed kind explicit

Today the kind is implicit: `SharedEmbedder::embed` (Priority::Low) is the corpus
path, `embed_query` (Priority::High) is the query path (`shared.rs:206-218`).
Priority and kind coincide *by accident*. Thread an explicit `EmbedKind`
(`Query` | `Document`) through the boundary so prompt selection does not
piggyback on a scheduler concept. The two existing methods map cleanly:
`embed` → `Document`, `embed_query` → `Query`. The `EmbeddingProducer` trait
(`llama.rs:201`, `remote.rs`) takes the kind so each backend applies the prompt.

## D3 — No invalidation in this change (query-only)

Query vectors are computed fresh per search and never persisted; the corpus
(document) side is untouched. So **no recipe bump, no re-embed, no migration** —
`CODE_TEXT_RECIPE` stays `"doc/v1"`.

The original coupling stands as a recorded trap for the deferred half: if the
**document** prompt is ever adopted, `embeddable_text` bytes won't move (the
prompt is applied inside the producer), so the fingerprint won't either — that
adoption MUST bump `CODE_TEXT_RECIPE` (and `FINDING_TEXT_RECIPE` iff findings
adopt it) or existing indexes silently keep stale prompt-less vectors. Note:
`reset_vectors`' wipe-on-mismatch behavior is being replaced by per-generation
dirs in `shared-vector-cache`; the recipe-bump obligation survives that change
(a bump then means "new generation dir", not "wipe").

## D4 — Exact prompt strings (confirmed)

The canonical EmbeddingGemma retrieval prompts (from the model's
`config_sentence_transformers.json` `prompts` map / model card), as used by the
A/B harness (`Q_PROMPT`/`D_PROMPT` in `prompt_ab.rs`):

| kind | prompt prefix | status |
|------|---------------|--------|
| query | `task: search result \| query: ` | **ships** |
| document | `title: none \| text: ` | deferred (D5 verdict) |

Store the query prompt as a named constant next to the producer.

## D5 — A/B eval harness (the gate for landing) — RAN, verdict in proposal.md

> Ran 2026-07-05 as `crates/kenn-store/examples/prompt_ab.rs` (200 queries,
> 7,363-symbol corpus). query-only won on r@1/MRR; query+doc added nothing and
> slightly hurt r@1 → per the pre-registered gate below, **query-only ships**.
> The fused-hybrid no-regression check was not run in the eval; it moves to the
> implementation's verification tasks (tasks.md §2.4).

The prompts land only if they measurably help kenn's vector arm. Reuse the
self-supervised eval approach already used for fusion work (the `rrf-*` /
`benchmark.md` methodology) rather than inventing a new corpus.

**Corpus:** an indexed real repo with doc prose — kenn itself is the honest,
shareable choice (no private codebases). Optionally a second public fixture.

**Labels (self-supervised):** for each symbol with a doc comment, treat the doc's
natural-language summary (or a held-out paraphrase of it) as the query and the
symbol as the single relevant target. This mirrors "find code by describing it",
which is the actual use case, and needs no hand labels.

**Arms — embed the corpus three ways into separate vector stores:**
1. `no-prompt` — current behavior (baseline).
2. `query-only` — query prompt on the search side, raw docs.
3. `query+doc` — both prompts (the proposed behavior).

**Metric:** recall@{1,5,10} and MRR on the **isolated vector arm** (bypass RRF
fusion) to measure the prompt effect cleanly; then re-check recall@10 on the
**fused hybrid** result to confirm no regression once lexical is blended back in.

**Decision gate:**
- Ship `query+doc` iff it beats `no-prompt` on vector-arm recall@10 / MRR by a
  margin that clears eval noise, **and** fused hybrid recall does not regress.
- If `query-only` captures most of the gain, prefer it (no corpus re-embed cost).
- If neither beats baseline on kenn's doc prose, close the change; record the
  null result so it is not re-litigated.

**Cost:** each doc-prompted arm re-embeds the whole corpus once. `just embed-smoke`
confirms in-sandbox embedding works, so the harness runs here without user
involvement. Isolate arms by `.kenn/vectors/` dir (or a temp store) per arm.

## D6 — Open questions (updated post-verdict)

- ~~Item-to-item ("similar symbols") search reuses a **committed** document
  vector as the query.~~ **Dissolved by query-only:** document vectors are
  unprompted, so item-to-item (raw doc vector against raw doc vectors) is
  byte-identical to today. Only free-text queries carry the prompt.
- Deferred with the document prompt (revisit only if a stronger eval reopens
  it): whether a real `title` beats `none` for code symbols, and whether the
  code-specific InstructionRetrieval prompt beats the generic `search result`
  task on the query side.
