## ADDED Requirements

### Requirement: The staleness key has a git form and a non-git form

A workspace's `StalenessKey` SHALL take one of two concrete forms — a
*git* form for a git repository, and a *tree-fingerprint* form for a
non-git workspace — plus an `Unknown` form for when neither can be
determined. `compute_staleness_key` SHALL return the git form when
`git rev-parse HEAD` succeeds, and the tree-fingerprint form otherwise.
The git form SHALL remain `(HEAD commit, sorted dirty-file hashes)` as
before this change.

#### Scenario: a git workspace yields the git form

- **GIVEN** a workspace that is a git repository with a commit
- **WHEN** `compute_staleness_key` runs
- **THEN** the key is the git form carrying the `HEAD` commit

#### Scenario: a non-git workspace yields the tree-fingerprint form

- **GIVEN** a workspace that is not a git repository
- **WHEN** `compute_staleness_key` runs
- **THEN** the key is the tree-fingerprint form

### Requirement: The non-git fingerprint is a stat-based tree digest

For a non-git workspace `compute_staleness_key` SHALL produce the
fingerprint from a `stat`-only depth-first walk of the source tree: each
file contributes its `(workspace-relative path, mtime, size)` to a
stable digest, visited in a deterministic order. The walk SHALL NOT read
file contents. The walk SHALL skip a fixed set of directory names —
`node_modules`, `target`, `bin`, `obj`, `.git`, and `.kenn` — so that
indexing's own output (written under `.kenn/`) never perturbs the
fingerprint. The walk SHALL NOT consult the configurable
`[exclude] globs`.

#### Scenario: editing a source file changes the fingerprint

- **GIVEN** a non-git workspace fingerprinted once
- **WHEN** a source file's contents change and it is fingerprinted again
- **THEN** the two fingerprints differ

#### Scenario: an unchanged tree yields a stable fingerprint

- **GIVEN** a non-git workspace that has not changed
- **WHEN** it is fingerprinted twice
- **THEN** the two fingerprints are equal

#### Scenario: the derived store does not perturb the fingerprint

- **GIVEN** a non-git workspace fingerprinted once
- **WHEN** an index run writes snapshots under `.kenn/`
- **THEN** the fingerprint is unchanged, because `.kenn/` is skipped

### Requirement: Staleness keys match only within the same form

`StalenessKey::matches` SHALL return true only when both keys have the
same form and equal contents: two git keys match iff their `HEAD` and
dirty-file sets are equal; two tree keys match iff their fingerprints
are equal. A git key and a tree key SHALL never match, and an `Unknown`
key SHALL never match anything — a non-match costs only one redundant,
always-safe reindex.

#### Scenario: equal tree fingerprints match

- **GIVEN** two tree-fingerprint keys with the same fingerprint
- **WHEN** they are compared with `matches`
- **THEN** the result is true

#### Scenario: a git key and a tree key never match

- **GIVEN** one git-form key and one tree-fingerprint key
- **WHEN** they are compared with `matches`
- **THEN** the result is false
