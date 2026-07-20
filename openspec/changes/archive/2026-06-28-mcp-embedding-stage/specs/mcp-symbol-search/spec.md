## MODIFIED Requirements

### Requirement: find_similar signals a missing committed vector

The `find_similar` tool SHALL distinguish a symbol with no committed vector from a
symbol that simply has no near neighbours, and SHALL further distinguish whether a
missing vector is **transient** (the embedding pass is still running) or
**terminal** (it has finished or no embedder exists). When the given symbol has no
committed vector:

- if the server's embedding stage is **building** (the background embed pass is in
  progress — `get_index_status` reports `state: "embedding"`), the tool SHALL return
  a **transient, retryable** error telling the agent embeddings are still building
  and to retry shortly;
- otherwise (the embed pass has finished, or no embedder is configured —
  `state: "ready"` or `"disabled"`), the tool SHALL return the **terminal**
  `EMBEDDING_UNAVAILABLE` error with an actionable message (`kenn embed`, or the
  symbol has no embeddable text).

In neither case SHALL it return an empty result. An empty result SHALL mean only
that the vector exists but no similar symbols were found. This lets an agent running
the `dup`/`audit` duplication leg wait for an in-progress embed pass instead of
mistaking "still building" for "no duplication."

#### Scenario: missing vector while embedding is transient

- **GIVEN** the server's `state` is `"embedding"` (the embed pass is running)
- **WHEN** `find_similar` is called for a symbol with no committed vector yet
- **THEN** it returns a transient, retryable error indicating embeddings are still
  building, not the terminal `EMBEDDING_UNAVAILABLE`

#### Scenario: missing vector after embedding is terminal

- **GIVEN** the server's `state` is `"ready"` or `"disabled"`
- **WHEN** `find_similar` is called for a symbol with no committed vector
- **THEN** it returns the terminal `EMBEDDING_UNAVAILABLE` error naming `kenn embed`

#### Scenario: a vectored symbol with no near neighbours returns empty

- **GIVEN** a symbol that has a committed vector but nothing similar in the corpus
- **WHEN** `find_similar` is called for it
- **THEN** it returns an empty result, not an error
