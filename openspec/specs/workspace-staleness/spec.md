# workspace-staleness Specification

## Purpose
TBD - created by archiving change non-git-staleness. Update Purpose after archive.
## Requirements
### Requirement: The staleness key has a git form and a non-git form

A workspace's `StalenessKey` SHALL take one of two concrete forms — a *git* form
for a git repository, and a *tree-fingerprint* form for a non-git workspace — plus
an `Unknown` form for when neither can be determined. `compute_staleness_key` SHALL
return the git form when the workspace is a git repository, and the tree-fingerprint
form otherwise. All git metadata for the git form (the HEAD commit and the
tracked-modified file set) SHALL be read **in-process via a git library**, NOT by
invoking the external `git` binary — so the git form does not depend on `git` being
present on `PATH`, on git's user/system config (e.g. `safe.directory`,
`core.quotepath`), or on a per-call process spawn.

The git form SHALL be `(HEAD commit, sorted hashes of tracked-modified files)`. It
SHALL hash **only tracked files reported modified** — it SHALL NOT read or hash
untracked files. This bounds the key's cost to the set of tracked changes and
prevents untracked scratch (e.g. `node_modules/`, build output, tmp clones) from
inflating the key or its compute time.

A tracked file reported **deleted** has no content to hash. It SHALL still
contribute an entry to the key — a fixed **deletion sentinel** — rather than be
dropped, so the deletion changes the key.

#### Scenario: a git workspace yields the git form

- **GIVEN** a workspace that is a git repository with a commit
- **WHEN** `compute_staleness_key` runs
- **THEN** the key is the git form carrying the `HEAD` commit

#### Scenario: the git form needs no git binary on PATH

- **GIVEN** a git repository and no `git` executable resolvable on `PATH`
- **WHEN** `compute_staleness_key` runs
- **THEN** it still returns the git form (HEAD + tracked-modified hashes)
- **AND** it does not fall back to the tree-fingerprint form

#### Scenario: untracked files do not affect the git key

- **GIVEN** a git workspace with a large untracked directory (e.g. `node_modules/`)
- **WHEN** `compute_staleness_key` runs
- **THEN** none of the untracked files are read or hashed
- **AND** adding or removing untracked files leaves the key unchanged

#### Scenario: a tracked modification changes the key

- **WHEN** a tracked, committed file is modified in the working tree
- **THEN** the git key's dirty-file hashes change so it no longer matches the prior key

#### Scenario: a tracked deletion changes the key

- **WHEN** a tracked, committed file is deleted from the working tree
- **THEN** the git key carries a deletion-sentinel entry for that path
- **AND** the key no longer matches the pre-delete key, so the reindex is not skipped

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

