## ADDED Requirements

### Requirement: find_similar signals a missing committed vector

The `find_similar` tool SHALL distinguish a symbol with no committed vector from a
symbol that simply has no near neighbours. When the given symbol has no committed
vector — the embeddings have not been built (`kenn embed` has not run or its
cold-start pass is still in progress), or the symbol has no embeddable text — the
tool SHALL return an `EMBEDDING_UNAVAILABLE` error with an actionable message, and
SHALL NOT return an empty result. An empty result SHALL mean only that the vector
exists but no similar symbols were found. This prevents an agent (e.g. running the
`dup`/`audit` duplication leg) from mistaking "vectors not built" for "no
duplication," which on a freshly-indexed repo silently produces nothing.

#### Scenario: a symbol with no committed vector errors actionably

- **GIVEN** an indexed repo whose embeddings have not been built
- **WHEN** `find_similar` is called for a symbol in it
- **THEN** it returns an `EMBEDDING_UNAVAILABLE` error naming `kenn embed`, not an
  empty result

#### Scenario: a vectored symbol with no near neighbours returns empty

- **GIVEN** a symbol that has a committed vector but nothing similar in the corpus
- **WHEN** `find_similar` is called for it
- **THEN** it returns an empty result, not an error
