## MODIFIED Requirements

### Requirement: The staleness key has a git form and a non-git form

A workspace's `StalenessKey` SHALL take one of two concrete forms — a
*git* form for a git repository, and a *tree-fingerprint* form for a
non-git workspace — plus an `Unknown` form for when neither can be
determined. `compute_staleness_key` SHALL return the git form when
`git rev-parse HEAD` succeeds, and the tree-fingerprint form otherwise.

The git form SHALL be `(HEAD commit, sorted hashes of tracked-modified
files)`. It SHALL hash **only tracked files reported modified** — it
SHALL NOT read or hash untracked files. This bounds the key's cost to
the set of tracked changes and prevents untracked scratch (e.g.
`node_modules/`, build output, tmp clones) from inflating the key or
its compute time. As a consequence, the git form does not observe
changes to gitignored generated files; those are covered by the file
watcher (which filters by source extension, not git status), not by
the staleness key.

A tracked file that `git status` reports **deleted** has no content to hash. It
SHALL still contribute an entry to the key — a fixed **deletion sentinel** —
rather than be dropped. Dropping it would leave the dirty set identical to the
clean pre-delete state (where the file was not dirty), so the key would match and
the reindex would be wrongly skipped; the sentinel makes the deletion change the
key.

#### Scenario: a git workspace yields the git form

- **GIVEN** a workspace that is a git repository with a commit
- **WHEN** `compute_staleness_key` runs
- **THEN** the key is the git form carrying the `HEAD` commit

#### Scenario: a non-git workspace yields the tree-fingerprint form

- **GIVEN** a workspace that is not a git repository
- **WHEN** `compute_staleness_key` runs
- **THEN** the key is the tree-fingerprint form

#### Scenario: untracked files do not affect the git key

- **GIVEN** a git workspace with a large untracked directory (e.g.
  `node_modules/`)
- **WHEN** `compute_staleness_key` runs
- **THEN** none of the untracked files are read or hashed
- **AND** adding or removing untracked files leaves the key unchanged

#### Scenario: a tracked modification changes the key

- **WHEN** a tracked, committed file is modified in the working tree
- **THEN** the git key's dirty-file hashes change so it no longer
  matches the prior key

#### Scenario: a tracked deletion changes the key

- **WHEN** a tracked, committed file is deleted from the working tree
- **THEN** the git key carries a deletion-sentinel entry for that path
- **AND** the key no longer matches the pre-delete key, so the reindex is not
  skipped
