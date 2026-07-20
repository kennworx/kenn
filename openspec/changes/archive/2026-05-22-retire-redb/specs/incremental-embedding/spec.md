## MODIFIED Requirements

### Requirement: Committed versus derived store layout

Within the code-intelligence store, `.kenn/vectors/` SHALL be the only committed artifact. The databases that store builds — the code-graph Lance datasets, the `knowledge/` Lance dataset, the BM25 indexes, and the IVF_PQ index — SHALL be classified as derived, gitignored, and rebuilt per worktree. There SHALL be no redb store. (The findings store, `.kenn/findings/`, is a separate durable Lance store on its own lifecycle — outside this requirement's scope; the `committed-findings` change governs its committed-versus-derived disposition.)

The derived Lance datasets — the code graph and the knowledge store — SHALL be co-located under `.kenn/local/` as one per-index-run snapshot: built into a single `building/` directory and published by one atomic directory swap. `.kenn/knowledge/` SHALL NOT remain a separate top-level path. `.kenn/.gitignore` therefore ignores `local/`, with `.kenn/vectors/` tracked as the committed embedding sidecar.

#### Scenario: a fresh worktree rebuilds derived state and reuses vectors

- **WHEN** a fresh git worktree or clone runs `kenn index`
- **THEN** the code-graph Lance datasets, the `knowledge/` Lance store, BM25, and IVF_PQ are rebuilt locally from source, and vectors are taken from the committed `.kenn/vectors/` sidecar — only that worktree's own diff is embedded

#### Scenario: derived datasets publish as one snapshot

- **WHEN** an index run finalizes
- **THEN** the code graph and the knowledge store are published together by a single atomic directory swap under `.kenn/local/`
- **AND** no derived Lance dataset remains at a separate top-level `.kenn/` path
