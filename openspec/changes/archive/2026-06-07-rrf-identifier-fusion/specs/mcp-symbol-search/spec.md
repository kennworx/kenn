## MODIFIED Requirements

### Requirement: search_symbols ranks by blended name + doc score

The blended symbol search SHALL fuse its retrieval arms by Reciprocal Rank
Fusion (rank-based), not by additive raw scores, and SHALL include the symbol's
`name_lower` identifier signal (exact/prefix/contains) as a fused arm so that one
search covers both exact-identifier and conceptual queries. An exact identifier match SHALL rank
first via an additive exact-name bonus applied on top of the fused score. The
conceptual (semantic) ranking of prose queries SHALL NOT regress relative to the
prior additive fusion.

The blended search SHALL NOT incorporate a graph-proximity arm that re-weights
or displaces already-ranked hits (evaluated and rejected: net-negative on
conceptual queries across corpora).

#### Scenario: exact identifier query ranks the named symbol first

- **GIVEN** a symbol whose `name_lower` equals the query
- **WHEN** the blended search runs for that identifier
- **THEN** that symbol is ranked first
- **AND** blended identifier recall is on par with `find_symbol_tiered`

#### Scenario: conceptual prose query does not regress

- **GIVEN** a prose query with no `name_lower` match
- **WHEN** the blended search runs
- **THEN** the identifier arms contribute nothing and the semantic ranking is
  unchanged from the prior fusion

#### Scenario: rank fusion replaces additive weights

- **WHEN** arms are combined
- **THEN** each arm contributes by reciprocal rank (`w / (K + rank)`)
- **AND** the prior additive `3 / 1 / 8` magnitude weights are no longer used

## ADDED Requirements

### Requirement: Identifier lookup is separator-agnostic

The identifier-lookup path (`find_symbol_tiered`) SHALL match identifiers by
their words independent of casing/separator style (camelCase, PascalCase,
snake_case). It SHALL split identifiers into lowercase words on both the index
and the query side and match them with a word tokenizer (not trigram), so a
multi-word query finds a symbol whether it is named `cancel_order`,
`CancelOrder`, or `cancel-order`. This word-split matching SHALL NOT be fused
into the blended conceptual search (it regresses conceptual ranking — see
design); blended's only identifier signal is the `name_lower` exact/prefix/
contains fold-in.

#### Scenario: snake_case symbol found by its words via identifier lookup

- **GIVEN** a symbol named `search_symbols_blended`
- **WHEN** `find_symbol_tiered` is queried with `search symbols blended`
- **THEN** that symbol is returned within the top results
- **AND** the same holds for the camelCase form `SearchSymbolsBlended`

#### Scenario: word-split matching does not pollute conceptual search

- **GIVEN** a prose/conceptual query to the blended search
- **THEN** identifier word-token matching does not displace the semantically
  correct result (the word-split arm is not part of blended fusion)

### Requirement: FTS5 queries are normalized through one safe builder

Every FTS5 arm SHALL build its MATCH expression through a single normalizer that
guarantees a valid, injection-safe expression for arbitrary input — including
hyphens, quotes, and operator words (`OR`, `NEAR`, `AND`). The normalizer SHALL
be tokenizer-aware: trigram arms match a quoted literal (substring search); word
arms split the query into tokens and combine them with OR, ranked by BM25 (not
AND).

#### Scenario: query with operator words and punctuation is valid

- **GIVEN** a query containing a hyphen or the word `OR`
- **WHEN** any FTS5 arm runs
- **THEN** the MATCH expression is valid and raises no syntax error
- **AND** the word arm matches by term rather than as one brittle phrase
