## MODIFIED Requirements

### Requirement: Committed versus derived store layout

Within the kenn store, `.kenn/vectors/` SHALL be the only committed artifact. The databases that store builds — the code-graph snapshot database, the search database, and the full-text and vector indexes inside them — SHALL be classified as derived, gitignored, and rebuilt per worktree. There SHALL be no redb store. (The findings store is a separate durable store on its own lifecycle — its committed per-finding records live under `.kenn/findings/` and its database at the derived root — outside this requirement's scope; the `committed-findings` change governs its committed-versus-derived disposition.)

The derived databases — the code graph and the search store — SHALL be co-located under `.kenn/local/` as one per-index-run snapshot: built into a single `building/` directory and published by one atomic directory swap. `.kenn/knowledge/` SHALL NOT remain a separate top-level path. `.kenn/.gitignore` therefore ignores `local/`, with `.kenn/vectors/` tracked as the committed embedding sidecar.

#### Scenario: a fresh worktree rebuilds derived state and reuses vectors

- **WHEN** a fresh git worktree or clone runs `kenn index`
- **THEN** the code-graph and search databases and their full-text and vector indexes are rebuilt locally from source, and vectors are taken from the committed `.kenn/vectors/` sidecar — only that worktree's own diff is embedded

#### Scenario: derived datasets publish as one snapshot

- **WHEN** an index run finalizes
- **THEN** the code graph and the search store are published together by a single atomic directory swap under `.kenn/local/`
- **AND** no derived database remains at a separate top-level `.kenn/` path
