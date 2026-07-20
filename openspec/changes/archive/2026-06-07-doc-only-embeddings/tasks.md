## 0. Recipe chosen by measurement (done — see design.md)

- [x] 0.1 `clean-doc-prose` implemented (C# sidecar emits plain prose) and the C#
      corpus reindexed — 0 docs retain markup, `name_words` built.
- [x] 0.2 Re-measured `sig+doc` vs doc-only on the cleaned C# corpus (valid prose
      gold): MRR 0.947 → 0.966. The prior contaminated numbers (recipe_spike +8%,
      composed_spike −62%) are discarded.
- [x] 0.3 Recipe decided: **doc-only + skip-undocumented** (doc-only ≥ sig+doc on
      all three corpora; fallback rejected). Recorded in design.md D1–D3.

## 1. Implement the doc-only embeddable recipe

- [x] 1.1 Change the `name`-row embeddable text from `sig\ndoc` to the doc prose
      only (`finalize` build_name_rows + the embed-text reconstruction in
      `db::jobs::scan_rows` must agree).
- [x] 1.2 Skip embedding symbols with no doc — no `vec0` row is written for them
      (they stay findable via the lexical arms).
- [x] 1.3 Base the embeddable-text fingerprint on the doc text only, so a
      signature-only source edit does not trigger a re-embed.

## 2. Reconcile + migrate

- [x] 2.1 Ensure search degrades correctly when a symbol has no vector (lexical
      arms only) — no errors, just no vector-arm contribution.
- [x] 2.2 `kenn update` (full re-embed) regenerates the corpus under the doc-only
      recipe and drops vectors for now-unembedded symbols.

## 3. Verification

- [x] 3.1 Documented-symbol conceptual recall ≥ the prior `sig+doc` recipe
      (measured in phase 0: Rust +19% in-fusion, TS tie, C# +2% on clean docs) —
      re-confirm against the implemented recipe.
- [x] 3.2 Vector count drops to ~the number of documented symbols.
- [x] 3.3 `cargo clippy --workspace --all-targets`; `just crap-ci`; `cargo fmt --all`.
