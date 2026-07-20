## Why

The vector arm embeds the `sig\ndoc` recipe for every symbol. That's wrong on
two axes — **the signature hurts conceptual recall**, and **embedding
undocumented symbols isn't worth it** — both now confirmed by measurement across
three corpora (design D1/D2).

- **The signature *hurts* conceptual recall.** EmbeddingGemma mean-pools tokens,
  so prepending signature tokens (`amount number rate number → number`) drags
  the vector toward type-space and away from the prose-space where conceptual
  queries live. Held-out conceptual recall (G2 MRR), `sig+doc → doc-only`:

  ```
   Rust (kenn, in-fusion)   0.756 → 0.899   (+19%)
   TypeScript               1.000 → 1.000   (tie, at ceiling)
   C# (clean, same pool)    0.947 → 0.966   (+2%)
  ```

  doc-only wins or ties everywhere. (An earlier C# "−62%" was an artifact of
  raw-XML doc storage; it vanished once `clean-doc-prose` made the docs plain
  prose — see design D1.)

- **Embedding undocumented symbols isn't worth it.** Once the
  `rrf-identifier-fusion` lexical arms own identifier + signature matching, an
  undocumented symbol's name-vector recovers only a weak, capped signal
  (recall@10 ≈ 0.56 vs the name-token BM25 arm's 0.19) while forcing every symbol
  to be embedded. Undocumented symbols stay findable via the lexical arms; the
  coverage tradeoff is accepted and recorded (design D2/D3).

This change **depends on `rrf-identifier-fusion`** (lexical arms must own
identifiers/signatures first) **and on `clean-doc-prose`** (doc-only on C# is
only valid once docs are plain prose, not raw `<member>` XML).

## What Changes

- The embeddable-text recipe becomes **doc-only**: embed the symbol's
  documentation prose, not `sig\ndoc`.
- **Only documented symbols are embedded.** A symbol with no doc gets no vector;
  it is found lexically (name-token / identifier / signature arms), not
  semantically. This is the bulk of the embedding-cost saving. A doc→sig
  fallback was considered and rejected (design D3).
- The embeddable-text fingerprint that drives incremental re-embedding tracks
  the doc text only, so a signature-only edit no longer forces a re-embed.

## Capabilities

### Modified Capabilities

- `incremental-embedding`: the embeddable-text recipe and fingerprint are
  doc-only; undocumented symbols are not embedded.

## Impact

- **Quality:** conceptual recall up — +19% G2 in-fusion on Rust, +2% on clean
  C#, tie on TS (signature noise removed).
- **Cost:** dramatically fewer vectors — embed only documented symbols
  (e.g. a TS corpus ~4,030 → 34; a C# corpus ~76k → ~3.1k after cleaning).
- **Coverage:** undocumented symbols rely on the lexical arms; the lever for
  their conceptual recall is writing docs (design D3).
- **Code:** the embeddable-text builder (`name`-row recipe) and its fingerprint
  in the search store + incremental-embedding manifest.
