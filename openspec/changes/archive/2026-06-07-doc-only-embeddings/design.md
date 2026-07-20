# Design — doc-only embeddable recipe

Decided 2026-06-07 from cross-corpus measurement, after `clean-doc-prose` made
C# docs plain prose. The recipe is **doc-only**: embed the symbol's documentation
prose, drop the signature, and skip symbols with no usable doc.

## D1 — doc-only ≥ sig+doc on conceptual recall, all three corpora

Held-out conceptual recall (G2 MRR), `sig+doc → doc-only`:

```
 Rust (kenn, in-fusion)   0.756 → 0.899   (+19%)
 TypeScript               1.000 → 1.000   (tie, at ceiling)
 C# (clean, same pool)    0.947 → 0.966   (+2%)
```

EmbeddingGemma mean-pools tokens, so prepending signature tokens drags the
vector toward type-space and away from the prose-space where conceptual queries
live. The earlier C# "−62% regression" was an artifact of raw-XML doc storage
(the doc-derived gold query was the `<member name=FQN>` boilerplate line); once
`clean-doc-prose` lands, C# agrees with Rust/TS. The signature is mild noise
everywhere — drop it.

## D2 — skip undocumented; the lexical arms cover them

A symbol with no doc has nothing conceptual to embed. Its name-vector recovers
only a weak signal (recall@10 ≈ 0.56 vs the name-token BM25 arm's 0.19), so the
`rrf-identifier-fusion` lexical arms (`name_lower`, word-split, signature
trigram) own identifier/signature lookup. Skipping undocumented symbols is the
bulk of the embedding-cost saving.

## D3 — coverage tradeoff (accepted, recorded, not hidden)

Pure doc-only shrinks the vector index to documented symbols only — on the C#
corpus, ~3.1k of 76k after cleaning. Undocumented symbols lose semantic/vector
search and rely on the lexical arms. Accepted: there is no conceptual signal to
embed for an undocumented symbol. A doc→sig **fallback was considered and
rejected** — it re-embeds every symbol (forfeiting the cost saving) only to
recover the weak name-vector signal D2 measured. The lever for an undocumented
symbol's conceptual recall is writing a doc, not embedding its signature.

## D4 — depends on `clean-doc-prose`

doc-only on C# is only valid once docs are plain prose; embedding raw `<member>`
XML doc-only would be worse than `sig+doc`. `clean-doc-prose` must land first.
This change also depends on `rrf-identifier-fusion` (the lexical arms must own
identifiers/signatures before the vector arm narrows to doc prose).
