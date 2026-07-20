## 1. `fts5_match` normalizer

- [x] 1.1 Add a shared `fts5_match` that produces injection-safe MATCH
      expressions, per tokenizer: trigram → quoted literal; word → split → quote
      each token → OR (design D8). Replace the inline alphanumeric-strip /
      phrase-wrap / raw-join transforms.
- [x] 1.2 Test: hyphens, quotes, and operator-words (`OR`, `NEAR`) all yield
      valid MATCH; word arm matches by term, not as one phrase.

## 2. RRF fusion in `search_symbols_blended`

- [x] 2.1 Compute each arm as an ordered candidate list (rank = position), not a
      raw-score accumulation (design D1, D5).
- [x] 2.2 Fuse by RRF `w / (K + rank)`, K=60, replacing the additive
      `NAME/DOC/VECTOR` weight sum (design D1).
- [x] 2.3 Add the additive exact-name bonus (`name_lower == query`), sized to
      dominate the RRF sum (design D2).

## 3. Identifier signal in blended + word-split in the tiered path

- [x] 3.1 Fold the `name_lower` arm into blended: exact → prefix → contains,
      whole-query, internal/non-test only (design D3). This is the ONLY
      identifier signal in blended.
- [x] 3.2 Add word-split matching to **`find_symbol_tiered`** (NOT blended):
      split identifiers (camelCase + snake_case → words), `unicode61`, OR + BM25
      via `fts5_match`. The composed test showed this arm regresses conceptual
      recall when fused into blended, so it lives in the identifier tool (design D6).
- [x] 3.3 Combine query tokens with OR, not AND (design D7).

## 4. Verification

- [x] 4.1 **End-to-end validation of the composed blended config** (acceptance
      gate). The composed test (`composed_spike.rs`) already established the
      final shape on the kenn corpus: `RRF{name_lower(exact/prefix/contains),
      sig, doc, vector} + exact-bonus` gives G1 parity (0.965 vs tiered 0.977)
      and zero G2 regression, while the word-split arm regressed G2 and was moved
      to `find_symbol_tiered`. Re-confirm this final blended config on a second
      corpus (G1 parity + G2 no-regress), and confirm `find_symbol_tiered` +
      word-split lifts multi-word/snake_case identifier recall there.
- [x] 4.2 Retire the `3 / 1 / 8` magic weights and any now-dead additive path.
- [x] 4.3 `cargo clippy --workspace --all-targets` to zero warnings; `just crap-ci`.
- [x] 4.4 `cargo fmt --all` as the final step.

## 5. Out of scope (recorded)

- [x] 5.1 Graph-proximity arm: evaluated and rejected (design D4). Do NOT add a
      flat graph RRF stream.
- [x] 5.2 AND-combining query tokens: rejected for ranking (design D7); see the
      `cnf-query-filter` future change for the precision-filter use case.
