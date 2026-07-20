## MODIFIED Requirements

### Requirement: Model-identity manifest gating

Vectors SHALL be stored **per generation**, where a generation is
`(model_id, dim, quant, recipe)`. Reconciliation SHALL reuse a committed vector
only when its generation matches the active embedder's generation; the generation
is reflected in the sidecar layout (a per-generation namespace) so that multiple
generations coexist.

A generation change (new model id, dim, quant, or recipe) SHALL write vectors into
a **new** generation namespace and SHALL NOT wipe or invalidate prior generations —
there is no destructive whole-directory reset. Switching back to a prior generation
SHALL reuse its retained vectors with no re-embedding.

The model id SHALL be a plain string (e.g. `"embeddinggemma-300M"`); model
upgrades are expressed by versioning the id. Provider URL and content hashes are
not recorded.

#### Scenario: matching generation reuses vectors

- **GIVEN** a sidecar generation for `(embeddinggemma-300M, 768, int8, doc/v1)`
- **WHEN** the active embedder matches that generation
- **THEN** reconciliation reuses those vectors

#### Scenario: a generation change is additive, not destructive

- **GIVEN** an existing generation `(embeddinggemma-300M, 768, int8, doc/v1)`
- **WHEN** the recipe changes to `doc-gemma/v2`
- **THEN** the new generation is written into its own namespace
- **AND** the `doc/v1` vectors remain intact and reusable
- **AND** no whole-directory reset occurs

#### Scenario: switching back reuses the retained generation

- **GIVEN** both `doc/v1` and `doc-gemma/v2` generations exist
- **WHEN** the active embedder reverts to `doc/v1`
- **THEN** its vectors are reused with zero re-embedding

## ADDED Requirements

### Requirement: The vector cache is garbage-collected

The vector store SHALL be garbage-collected so generations do not accumulate
unbounded (they span worktrees/projects when shared): it SHALL track
per-generation last-access time and evict least-recently-used generations past a
configurable size cap. GC SHALL be the only operation requiring a lock on the
vectors root (content-addressed appends remain lock-free), and it SHALL be
triggerable lazily (at index start) and explicitly (a `kenn gc` command).

#### Scenario: an idle generation is evicted under size pressure

- **GIVEN** the vector store exceeds its configured size cap
- **AND** a generation has not been accessed most recently
- **WHEN** garbage collection runs
- **THEN** that generation's vectors are evicted
- **AND** the active generation's vectors are retained

#### Scenario: appends do not block on GC

- **WHEN** a content-addressed vector append occurs concurrently with normal use
- **THEN** it proceeds without acquiring the GC lock
