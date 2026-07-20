# Design — RRF identifier-unified fusion

Validated by a throwaway benchmark harness (`crates/kenn-store/examples/
fusion_spike.rs`, `tokenize_spike.rs`) over three corpora with self-supervised
gold sets:

- **G1 identifier→def** — query = a symbol's exact name; gold = that symbol.
- **G2 doc→def (held-out)** — query = a symbol's doc prose; the doc arm holds
  the gold out, so recall must come from the semantic (vector) arm.
- **G3 graph-recall** — gold is a graph neighbour, far in vector + lexical space.

Variants: **V0** = additive (`3 / 1 / 8`); **V1** = RRF + exact-bonus; **V1i** =
V1 + `name_lower` arm; **V2/V3** = V1i + graph arm (default / extended kinds).

## Cross-corpus results

```
                          Rust (this repo)  C# corpus        TS corpus
 G1 ident  V0 → V1i mrr   0.550 → 0.965     0.596 → 1.000    0.134 → 0.983
 G1  V1i vs tiered mrr    0.965 / 0.977     1.000 / 1.000    0.983 / 0.993
 G2 concept V1i vs V0     tie (0.939)       tie (0.939)      tie (1.000)
```

**D1 — RRF over additive.** Fuse by `w / (K + rank)`, K=60, not raw score.
Eliminates the BM25/cosine scale fight; weights become rank-space-robust instead
of the fragile `3 / 1 / 8` magnitudes.

**D2 — Additive exact-bonus keeps magnitude where it matters.** Pure RRF
discards magnitude, so an exact identifier would tie a mediocre rank-1. Add a
constant `name_lower == query` bonus, sized to dominate the max RRF sum
(≈ Σw/(K+1) ≈ 0.03; bonus = 1.0).

**D3 — Fold in the `name_lower` identifier signal.** Blended's "name arm" is a
*signature* index; the real identifier signal sits outside blended entirely.
Folding it in (exact → prefix → contains, whole-query, quiet on prose) makes one
search match the dedicated identifier tool (V1i ≈ tiered on G1, all corpora)
with conceptual recall unchanged (G2 V1i = V0 exactly).

**D4 — Graph arm: evaluated and REJECTED.** A graph-proximity arm (expand 1 hop
on Calls/Implements/Overrides from the top-M hits, fold neighbours in as a
weighted RRF stream) is net-negative on **every** corpus:

```
 G2 conceptual mrr (V0 → V2 graph):   0.765→0.631  |  0.939→0.674  |  1.000→0.758
 weight sweep: any graph_w that lifts G3 without wrecking G2?   NONE on any corpus
```

A flat graph stream can't tell "this neighbour is the only path to the answer"
from "this neighbour is noise next to an already-correct hit." Density makes it
worse (the 413k-edge corpus regressed hardest). Extended kinds (V3) were worse
than default everywhere. A future graph arm must be recall-only / conditional;
out of scope here.

**D5 — Per-arm rank requires decomposed arms.** RRF needs each arm's rank, so
arms are computed as ordered candidate lists and fused — an internal refactor of
`search_symbols_blended`, not a new public API.

**D6 — Identifier matching must word-tokenize both sides (separator-agnostic).**
FTS5 trigram is separator-*sensitive* (it indexes 3-char windows including
spaces/punctuation), so the current alphanumeric-strip only matches
separator-free text. Verified, finding a symbol by its own name-words:

```
 arm / style              r@1    r@10
 A trigram/strip  SNAKE   0.01   0.01   ← current normalization, snake_case
 A trigram/strip  CAMEL   0.88   0.99   ← only camelCase
 B trigram/spaced both    0.00   0.00   ← no fixed separator works
 C word-split     SNAKE   0.74   1.00   ← split + unicode61: works for both
 C word-split     CAMEL   0.86   1.00
```

So word-split matching splits identifiers into words (camelCase + snake_case +
punctuation → spaces) on **both** index and query side and uses `unicode61`, not
trigram. Separator style then becomes irrelevant.

**Where it lives: `find_symbol_tiered`, NOT the blended fusion.** The composed
end-to-end test (`composed_spike.rs`, task 4.1) measured the word-split arm
*inside* the full fusion and it Pareto-regressed conceptual queries — a prose
query matches any symbol sharing name-words, flooding out the vector hit:

```
              G1b multi-word id    G2 conceptual
 no token arm        0.411              0.756
 W_token=0.1         0.505              0.662   (G2 −12%)
 W_token=0.2         0.542              0.581   (G2 −23%)
 W_token=1.0         0.661              0.405   (G2 −46%)
```

No weight helps identifiers without hurting concepts — the graph-arm failure
mode again. So word-split matching goes into the **identifier** tool
(`find_symbol_tiered`), where multi-word/separator-variant identifier lookup
belongs and cannot pollute conceptual ranking. Blended's only identifier signal
stays the `name_lower` *exact/prefix/contains* fold-in (D3), which is what was
actually validated end-to-end (G1 parity, G2 no regression).

**D7 — Combine query tokens with OR + BM25, not AND.** Verified on the word-split
index:

```
 arm           exact r@1   noisy r@1 (one extra word)
 OR            0.77        0.77
 AND           0.77        0.00   ← collapses: AND needs every token
 AND→OR        0.77        0.77   ← == OR
```

AND gives no ranking gain (BM25-over-OR already surfaces full-coverage matches)
and is brittle to any extra/synonym word. The residual r@1 ceiling (0.77) is
genuine ties — overloads / same-named symbols — which the exact-bonus + the
`name_lower` exact arm resolve, not AND. AND only makes sense as a separate
precision *filter* (see the `cnf-query-filter` future change).

**D8 — One `fts5_match` normalizer, per tokenizer.** Replace the three ad-hoc
inline transforms (alphanumeric-strip, quote-strip-wrap, raw OR-join — the last
panicked on `non-fatal`) with a single normalizer: trigram arms quote the
literal (substring search); word arms split → quote each token → OR. Every arm
then receives an injection-safe, semantically-correct MATCH expression.

## Validity caveats

The arm numbers come from separate harnesses. The composition was then tested
end-to-end (`composed_spike.rs`) — and the test **changed the design**: the
word-split name-token arm regressed G2 inside the fusion (D6), so it was moved
out of blended into `find_symbol_tiered`. The *blended* config that remains —
`RRF{name_lower(exact/prefix/contains), sig, doc, vector} + exact-bonus` — is the
one validated end-to-end (G1 parity, G2 no regression). Task 4.1 re-confirms the
final blended config on a second corpus.

Remaining honesty notes:

- D7's "AND = OR" was measured on the *tautological* gold (query = a symbol's own
  name-words); it is sound for identifier-words queries but is not direct
  evidence for arbitrary conceptual queries. Recorded as rationale for OR + BM25,
  not a universal claim.
- doc-only embeddings (`doc-only-embeddings` change) were validated vector-arm-
  isolated; their effect inside the full blended fusion is re-confirmed by that
  change's task 3.1.
