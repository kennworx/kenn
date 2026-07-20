## MODIFIED Requirements

### Requirement: get_index_status returns lifecycle state

The `get_index_status` tool's response payload SHALL include a `state`
string field with one of `"indexing"`, `"embedding"`, `"ready"`, `"disabled"`,
or `"failed"`. These form the pipeline progression `indexing → embedding → ready`,
where `embedding` is the window in which the code graph is built but the background
embedding pass is still filling vectors, `disabled` replaces the `embedding → ready`
arc when no embedder is configured (vectors will not be built), and `failed` is a
cold-start index failure.

**Structural-vs-vector contract.** From the `embedding` stage onward the code graph
is queryable: structural tools (`find_symbol`, `find_usages`, `list_callers`, etc.)
SHALL succeed during `embedding`, `ready`, and `disabled`. Only vector tools
(`find_similar`, `semantic_search`) depend on the embedding pass. An agent that needs
only structural queries SHALL NOT wait for `ready` — `embedding` is sufficient. The
`embedding` and `disabled` states therefore behave like `ready` for the
not-Ready fast-fail gate (structural tools serve; they are not blocked).

When `state` is `"indexing"`, the payload SHALL include a `progress`
object with at least:
- `phase` (string) — current pipeline phase identifier
- `files_seen` (number)
- `symbols_seen` (number)

When `state` is `"failed"`, the payload SHALL include an `error`
string describing the failure.

When `state` is `"embedding"`, `"ready"`, or `"disabled"`, the existing fields
(`snapshot_id`, `indexed_at`, `is_stale`, `reindex_in_progress`,
`fallback_from_parent_worktree`) SHALL all be populated as in the prior `"ready"`
payload.

#### Scenario: Status during indexing carries progress

- **GIVEN** the server is in `Indexing` and has processed two batches
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "indexing"`
- **AND** `progress.phase` is a non-empty string
- **AND** `progress.files_seen` and `progress.symbols_seen` are
  non-negative numbers

#### Scenario: Status after failure carries error

- **GIVEN** the server is in `Failed` because the indexer subprocess
  exited with a non-zero status
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "failed"`
- **AND** `error` is a non-empty string describing the failure

#### Scenario: Status reports embedding while the background pass runs

- **GIVEN** the code graph is built and the background embed pass is running
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "embedding"`
- **AND** structural tools (e.g. `find_symbol`) succeed rather than fail-fast

#### Scenario: Status reports disabled when no embedder is configured

- **GIVEN** the code graph is built and no embedder is configured
- **WHEN** the embed pass completes
- **THEN** `get_index_status` reports `state: "disabled"`
- **AND** structural tools still succeed
