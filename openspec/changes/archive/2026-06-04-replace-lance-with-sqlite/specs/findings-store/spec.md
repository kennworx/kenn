## MODIFIED Requirements

### Requirement: the findings derived store and embeddings are derived

The findings derived store SHALL be a SQLite database, rebuilt from the committed `<id>.json`
records, and gitignored, never committed. A finding's embedding SHALL be stored in a dedicated
findings vector sidecar (`.kenn/findings/vectors/`) that reuses the `incremental-embedding`
sidecar format, keyed by the fingerprint of its `text`, and reconciled into the rebuilt store
on open; it SHALL NOT be persisted in a committed embedding column. The findings sidecar SHALL
be separate from the code sidecar — they have independent compaction live-sets and independent
manifests. The findings store SHALL use no storage engine other than SQLite.

The rebuild SHALL be atomic — a crash mid-rebuild SHALL leave the prior derived store usable —
and SHALL be serialized so concurrent opens rebuild at most once.

#### Scenario: a fresh open rebuilds the SQLite store from records

- **WHEN** `FindingsStore` opens against a `.kenn/findings/` directory of `<id>.json` records
- **THEN** the findings SQLite store is rebuilt from those records
- **AND** each finding's embedding is reconciled from the findings sidecar by the fingerprint of its text

#### Scenario: a new finding's embedding reaches the sidecar, not a committed column

- **WHEN** a finding with previously unseen text is flushed
- **THEN** its vector is appended to the committed `.kenn/findings/vectors/` sidecar
- **AND** no embedding is written to a git-committed embedding column

#### Scenario: a crashed rebuild leaves the prior store intact

- **WHEN** a rebuild of the derived findings store is interrupted before it completes
- **THEN** the previously built derived store remains usable
- **AND** the next `open` redoes the rebuild

#### Scenario: concurrent opens rebuild at most once

- **WHEN** two processes `open` the findings store at the same time and both find the staleness gate fired
- **THEN** exactly one rebuild runs and the other reuses its result
