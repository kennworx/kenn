# Embedding benchmark — evidence for the runtime (D2) and model choice

Evidence behind **D2 (runtime)**, **D4 (dimension)**, and the multilingual model
choice in `design.md`. Two phases: pick the runtime, then pick the model.

- **Hardware:** Apple M2 Pro, Metal GPU, unified memory.
- **Deployment target: Metal.** CPU numbers were collected early as a control
  but the production device is Metal — the comparisons below are Metal-only.
- **Corpus:** 2000 short text strings (code signatures + doc comments).
- **Measured:** embed-only throughput, peak process RSS, cross-runtime cosine
  agreement (an output-validity gate — a fast runtime that emits wrong vectors
  is disqualified).
- **Date:** 2026-05-21.

---

# Phase 1 — Runtime selection

Benchmark of every runtime that can plausibly run an embedding model in Rust or
behind a local API, on nomic-embed-text-v1.5 (Metal).

| Runtime | Throughput | Peak RSS | Valid? |
|---|--:|--:|---|
| **llama.cpp in-process** (`llama-cpp-2`) | **549.9 t/s** | **260 MB** | ✓ 0.998 |
| Candle — q8 port¹ | 320.2 t/s | 429 MB | ✓ 0.999 |
| Candle — fp32, tuned² | 328.2 t/s | 1081 MB | ✓ 1.000 |
| llama.cpp via local HTTP server | 172.5 t/s | 146 MB³ | ✓ 0.999 |
| ONNX (fastembed/`ort`) — CPU⁴ | 64.3 t/s | 4046 MB | ref |
| ONNX (fastembed/`ort`) — CoreML EP | 3.2 t/s | 9423 MB | ✓ 1.000 |
| Burn (`burn-import`, bge-base) | 22.7 t/s | 921 MB | ✓ 1.000 |

¹ Candle "q8 port" — a hand-written quantized model (ported from
`candle-transformers`' fp32 model, every `Linear` → `QMatMul`), loading the same
q8_0 GGUF llama.cpp uses. See finding 8.
² Candle "tuned" — the `accelerate` feature + length-sorted batching; defaults
measured 2–4× lower (finding 7).
³ HTTP-server memory is the server's *reported model size*, not process RSS.
⁴ ONNX/fastembed exposes no working GPU path on this hardware — its CoreML EP
collapses (finding 3) — so its best number is CPU.

**Winner: llama.cpp in-process via `llama-cpp-2`** — fastest and leanest, no
server or daemon, compiles from source as an ordinary cargo dependency.

## Findings (Phase 1)

### 1. Benchmark the runtime in-process — HTTP hid the truth
The local HTTP-server path measured llama.cpp at ~175 t/s. That was an artifact:
HTTP + JSON transport and the server's batching capped throughput at ~175 t/s.
The same engine in-process via `llama-cpp-2` hits **550 t/s** — byte-equivalent
output (cosine 1.0000). A local embedding server measured over HTTP understates
its runtime ~3×.

### 2. GPU helps — but how much depends on the CPU path
In-process llama.cpp: Metal 550 vs CPU 189 t/s — a real 2.9× GPU speedup. Tuned
Candle does Metal 328 vs CPU 246 — only 1.3×, because Accelerate BLAS makes its
CPU path fast. The GPU margin is not fixed. (Production uses Metal regardless.)

### 3. The CoreML execution provider is a non-starter for ONNX embedding
3.2 t/s — 15–20× slower than plain CPU ONNX — with memory exploding to 9–20 GB.
Numerically correct, but it cannot handle dynamic-shape BERT graphs. ONNX has no
usable GPU path here.

### 4. EmbeddingGemma on llama.cpp: the GGUF *conversion* matters, not the model
A third-party GGUF (the `unsloth` / LM Studio conversion) scored cosine **−0.018**
vs the ONNX reference — garbage, because it dropped the **dense projection head**
(768→3072→768). The model is fine: the **official `ggml-org` EmbeddingGemma
GGUF**, which keeps the dense head, scores cosine **0.999** and runs at **635 t/s
on Metal** (see Phase 2). The validity gate caught a broken *conversion*, not a
broken model. Always validate a community GGUF against an independent impl.

### 5. Quantization is nearly free on accuracy, huge on memory
GGUF q8_0 vs fp32: cosine 0.998 — negligible drift. In-process llama.cpp holds
the whole model + activations in 260 MB; ONNX needs 4 GB for the same model.

### 6. `burn-import` is numerically exact but Burn is the slowest runtime
`burn-import` imported the bge-base ONNX with zero unsupported ops; output exact
(1.000). But Burn is last on speed (~23 t/s) — the generated graph is unfused.

### 7. Default config is not a fair benchmark — Candle was understated 2–4×
Candle's defaults (no `accelerate`, batch-longest padding) measured 2–4× low.
A runtime's headline number is only as good as its build flags.

### 8. Quantization in Candle: a GPU win, a CPU disaster
A hand-written q8_0 port cut Candle's Metal memory 1081→429 MB at no speed cost.
The same port on CPU drops to 15 t/s: Candle's CPU q8_0 `QMatMul` dequantizes per
call and cannot use Accelerate. llama.cpp's q8 kernels are fast on both devices.

---

# Phase 2 — Multilingual model selection

The runtime is fixed (llama.cpp in-process). nomic-embed-text-v1.5 — the Phase-1
reference — is **English-only**, so it cannot be the model if multilingual
retrieval is required. Phase 2 compares multilingual models with **≥2048 context**
on llama.cpp/Metal. Validity reference for each: an independent implementation
(ONNX, or the Candle `QMatMul` port) — the EmbeddingGemma lesson (finding 4) means
every community GGUF must be checked.

| Model | dim | ctx | langs | Metal t/s | Metal RSS | Validity |
|---|--:|--:|--:|--:|--:|---|
| **EmbeddingGemma-300M** (ggml-org GGUF) | 768 | 2048 | 100+ | **635.5** | **794 MB** | ✓ 0.999 vs ONNX |
| BGE-M3 | 1024 | 8192 | 100+ | 251.0 | 1222 MB | ✓ 0.9996 vs Candle port |
| jina-embeddings-v3 (+ retrieval LoRA) | 1024 | 8192 | 89 | 196.4 | 1198 MB | LoRA applied; not independently verified |
| gte-multilingual-base | 768 | 8192 | 70+ | — | — | no embedding GGUF exists (only a reranker) |

All on llama.cpp in-process, q8_0, Metal.

### Notes

- **EmbeddingGemma** uses the **official `ggml-org/embeddinggemma-300M-GGUF`** —
  *not* the `unsloth` conversion, which is broken (finding 4). Validated to cosine
  0.999 against the ONNX reference.
- **BGE-M3** GGUF declares CLS pooling; cross-validated to cosine 0.9996 against
  an independent Candle `QMatMul` port of the same q8_0 weights.
- **jina-v3** ships as a base GGUF + 5 separate task LoRA adapters; the
  `retrieval.passage` adapter must be loaded and applied. We confirmed the adapter
  *is* applied (base-only vs base+LoRA cosine 0.83), but no independent reference
  implementation was available to validate the conversion itself — treat with
  caution.
- **gte-multilingual-base** has no embedding GGUF on the Hub — the only published
  GGUF is the *reranker* (a cross-encoder, not an embedding model). Not runnable
  on llama.cpp without a manual conversion.
- **nomic-embed-text-v2-moe** — the multilingual nomic (768-dim, ~100 languages,
  MoE) — was excluded by the context requirement: its window is only **512
  tokens**, below the 2048-token minimum. (The `nomic-v1.5` line in the table is
  the English-only variant.)

### Candle cross-check

For BGE-M3, the candle-vs-llama.cpp head-to-head (Metal, same q8_0 GGUF):

| | llama.cpp | Candle q8 |
|---|--:|--:|
| Throughput | **251 t/s** | 99 t/s |
| Peak RSS | **1222 MB** | 3626 MB |

llama.cpp wins decisively again — consistent with Phase 1. Candle remains the
pure-Rust fallback, not the performance choice.

---

# Phase 3 — Query-side embedder

Indexing embeds millions of texts in batches; a *query* embeds one short string,
sporadically. Different problem — measured separately. EmbeddingGemma-300M,
single-query workload (8-text corpus), peak RSS:

| Quant | CPU | Metal |
|---|--:|--:|
| q8 (ggml-org) | 867 MB | **558 MB** |
| q4 (ggml-org qat) | 768 MB | 506 MB |

q4-vs-q8 vector agreement: cosine **−0.018** — the q4 GGUF is broken.

### Findings (Phase 3)

- **Metal uses less memory than CPU for the query embedder** (558 vs 867 MB).
  The llama.cpp CPU backend repacks quantized weights into a CPU layout — ~300 MB
  of extra RAM. The query embedder should run on Metal, same as indexing.
- **q4 is a dead end.** It saves only ~50 MB — EmbeddingGemma's token-embedding
  table stays high-precision, so the q4 GGUF (277 MB) is barely smaller than q8
  (321 MB). And the available q4 GGUF (`ggml-org` qat-q4_0) is *broken* — cosine
  −0.018, the same dropped-dense-head signature as the broken `unsloth` q8.
- Device and quant cannot meaningfully move the ~558 MB loaded cost. The only
  real lever is **not keeping the model loaded**: lazy-load on the first vector
  query, unload after an idle TTL → steady-state ≈ 0, ~558 MB only during an
  active burst, ~1–3 s cold start after idle. See design D7.

---

## Conclusion

- **Runtime (D2):** llama.cpp in-process via `llama-cpp-2`.
- **Device:** Metal.
- **Dimension (D4):** 768.
- **Model:** **EmbeddingGemma-300M**, q8_0, the **official `ggml-org` GGUF**.
  It is the fastest and leanest multilingual model measured (635 t/s, 794 MB),
  the only **768-dim** one (so D4's 768 holds — no migration), covers **100+
  languages**, and meets the 2048-context minimum. BGE-M3 and jina-v3 are valid
  multilingual models but 2.5–3× slower and heavier (567M/570M params vs 300M),
  1024-dim (a dimension change), and jina-v3 carries unresolved GGUF-validity
  risk.

If multilingual retrieval is *not* required, nomic-embed-text-v1.5 q8_0 remains
the faster English-only option (550 t/s, 260 MB) — same runtime, same 768 dim.
