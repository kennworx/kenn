## 0. Gate — DONE (measured 2026-07-05, verdict: query-only)

- [x] 0.1 Build the A/B eval harness (design D5): self-supervised doc→symbol
      labels over kenn's own index; recall@{1,5,10} + MRR on the isolated vector
      arm. Landed as `crates/kenn-store/examples/prompt_ab.rs`.
- [x] 0.2 Run three arms — {no-prompt, query-only, query+doc} — isolating each in
      its own vector set, with the D4 prompt strings (confirmed against the
      model card; hardcoded as `Q_PROMPT`/`D_PROMPT` in the harness).
- [x] 0.3 Decision: **adopt query-only** (r@1 0.660 → 0.695, MRR 0.790 → 0.808;
      free — no re-embed). The document prompt adds nothing over query-only and
      slightly hurts r@1, so its cost (corpus re-embed + recipe bump) is not
      justified — **deferred** pending a stronger eval. Numbers in proposal.md.

## 1. Embed kind at the boundary

- [x] 1.1 Add `EmbedKind { Query, Document }`; thread it through the
      `EmbeddingProducer` trait (`llama.rs`, `remote.rs`), the scheduler
      (`Job.kind` → `BatchEncoder::encode_batch`), `LazyEmbedder`, and
      `SharedEmbedder::embed*` (`shared.rs`). `embed` → `Document`, `embed_query`
      → `Query`. Scheduler priority stays orthogonal. The kenn-server
      `/v1/embeddings` handler submits `Document` (raw pass-through — clients
      apply prompts before the wire, so the daemon prompting would double-prompt).
- [x] 1.2 Model-keyed prompt application implemented once
      (`task_prompt(model_id, kind)` in `producer.rs`, `contains("embeddinggemma")`
      case-insensitive), shared by both producers: `LlamaEmbedder::embed_sync`
      prepends at tokenize time; `RemoteEmbedder::embed` prepends client-side
      before the request. `Document` and non-EmbeddingGemma → raw.
      `embeddable_text` stays clean (D1).

## 2. Verification

- [x] 2.1 Real-weights test (`llama_integration.rs::query_kind_prompts_and_document_kind_stays_raw`,
      via `just embed-smoke`): Query vs Document vectors differ, and the Query
      vector is exactly equal to a Document embed of the manually-prompted text
      (the prompt is a pure prefix — nothing else changed).
- [x] 2.2 Wire-level tests (`remote.rs`): a non-EmbeddingGemma model sends raw
      text for both kinds; gemma + Document sends raw; gemma + Query sends the
      prompted string.
- [x] 2.3 No corpus invalidation: the 2.1 equivalence + the raw-Document wire
      test show document output is byte-identical; no recipe bump (design D3).
- [x] 2.4 Shipped-path check ran (`crates/kenn-store/examples/fused_ab.rs`,
      kenn's live index, 7,206 committed vectors): N=1000 fused hybrid —
      raw 0.621/0.846/0.711 vs prompted 0.619/0.845/0.711 (r@1/r@10/MRR@10).
      **No regression** (Δ ≤ 0.002, within noise); the isolated vector-arm gain
      washes to neutral in fusion on this gold set because doc-first-line
      queries already hit the lexical arms. The prompt ships on the vector-arm
      gain (free at runtime); note this gold set is lexically biased, so the
      fused delta understates the effect on genuinely paraphrased queries.
- [x] 2.5 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
      `cargo fmt --all` last.
