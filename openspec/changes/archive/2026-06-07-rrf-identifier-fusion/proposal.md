## Why

`search_symbols_blended` — the conceptual code search behind the MCP search
tool — fuses three arms (signature trigram `name_fts`, doc prose `doc_fts`,
`vec0` cosine) by **raw additive scores** with hand-tuned weights `3 / 1 / 8`.
A cross-corpus spike (this repo + two private corpora; Rust / C# / TS;
4k–69k symbols, 21k–413k edges) surfaced three problems:

- **Scale fight.** Additive fusion sums unbounded BM25 with bounded cosine; the
  `3 / 1 / 8` constants are fragile duct tape holding the scales together.
- **The identifier signal is siloed.** `name_fts` indexes the *signature*
  (`name_text` = param/type tokens), not the symbol name. Exact-identifier
  lookup lives in a *separate* path (`find_symbol_tiered` over `name_lower`). So
  a blended search for an exact identifier scores poorly — MRR **0.13–0.60**
  across corpora — while the dedicated identifier tool nails it (0.98–1.00).
- **Identifier matching is separator-broken.** The name arm strips queries to
  alphanumerics (`cancel-order` → `cancelorder`) and trigram-matches. Trigram is
  separator-*sensitive*, so this only works for camelCase. Measured recall@10
  for finding a symbol by its own words: **camelCase 0.99, snake_case 0.01** —
  i.e. snake_case identifiers (all Rust/Python/Ruby) are essentially unfindable.

## What Changes

- Replace additive fusion with **Reciprocal Rank Fusion** (RRF, K=60) over the
  arms, plus an additive **exact-name bonus** so an exact identifier always
  ranks first. RRF fuses by rank, killing the BM25-vs-cosine scale mismatch and
  retiring the `3 / 1 / 8` weights.
- **Fold the `name_lower` identifier signal into the fusion** (exact → prefix →
  contains), unifying conceptual search with identifier lookup: one search
  serves both "the symbol named X" and "code about X".
- Add **separator-agnostic word-split identifier matching to
  `find_symbol_tiered`** (NOT blended): split identifiers into words (camelCase +
  snake_case → lowercase words) on both index and query side, matched with a
  `unicode61` word tokenizer. This makes snake_case identifiers findable by their
  words — see design D6 for why this lives in the identifier path, not the
  blended fusion.
- Introduce a single **`fts5_match` query normalizer** so every FTS5 arm gets a
  valid MATCH expression (no more ad-hoc alphanumeric-strip / phrase-wrap that
  break on hyphens, quotes, or operator-words). It is per-tokenizer: trigram
  arms quote the literal; word arms split → quote each token → OR.
- Scope: `search_symbols_blended` (RRF + `name_lower`-exact fold-in) and
  `find_symbol_tiered` (word-split matching).
- **Rejected, recorded in design:** (1) a graph-proximity arm — net-negative on
  every corpus; (2) AND-combining query tokens — no ranking gain over BM25-OR;
  (3) folding the word-split name-token arm into the *blended* fusion — the
  composed test showed it Pareto-regresses conceptual recall (G2 −46% at full
  weight, still −12% at the lowest weight that helps identifiers).

## Capabilities

### Modified Capabilities

- `mcp-symbol-search`: blended symbol search becomes a separator-agnostic,
  rank-fused, identifier-unified ranking that covers identifier and conceptual
  queries in one search.

## Impact

- **Behavior:** blended identifier recall reaches `find_symbol_tiered` parity
  (MRR → 0.98–1.00) with **zero** conceptual-query regression; snake_case
  identifiers go from ~1% to fully findable by their words.
- **Code:** `crates/kenn-store/src/db/sqlite/reader/search.rs`
  (`search_symbols_blended`), `reader/projection.rs` (weights/constants), a new
  `name_lower` + word-split name-token arm over `graph.db`, and a shared
  `fts5_match` normalizer.
- **Enables:** doc-only embeddings (`doc-only-embeddings` change) — once the
  lexical arms own identifiers and signatures, the vector arm need only carry
  doc prose.
