## MODIFIED Requirements

### Requirement: A finding is a durable, provenance-bearing record

The store SHALL persist a finding as a committed per-finding record file —
`.kenn/findings/<id>.json` — carrying a unique `id`, free-form `text`, a list
of free-form `tags`, a list of `parent_ids`, and a `created_at` timestamp.
`parent_ids` SHALL be drawn from a single ID space shared with code-graph
nodes, so a parent MAY be a code-graph node or another finding. The finding's
embedding SHALL NOT be part of the record — it is derived (see the requirement
"the findings Lance store and embeddings are derived").

The store SHALL NOT impose a fixed `kind` enumeration on findings;
classification is carried by `tags`.

#### Scenario: a finding round-trips with its provenance

- **WHEN** a finding is stored with text, tags, and `parent_ids` referencing a code-graph node and an earlier finding
- **THEN** retrieving it by `id` returns the same text, tags, and `parent_ids`

#### Scenario: a finding record is a committed text file

- **WHEN** a finding is flushed
- **THEN** a `.kenn/findings/<id>.json` file holds its `id`, `text`, `tags`, `parent_ids`, and `created_at`
- **AND** that file is git-tracked, while the findings Lance store is not

## ADDED Requirements

### Requirement: the findings Lance store and embeddings are derived

The findings Lance dataset SHALL be derived — rebuilt from the committed
`<id>.json` records — and gitignored, never committed. A finding's embedding
SHALL be stored in a dedicated findings vector sidecar
(`.kenn/findings/vectors/`) that reuses the `incremental-embedding` sidecar
format, keyed by the fingerprint of its `text`, and reconciled into the rebuilt
Lance store on open; it SHALL NOT be persisted in a committed Lance column.
The findings sidecar SHALL be separate from the code sidecar — they have
independent compaction live-sets and independent manifests.

The rebuild SHALL be atomic — a crash mid-rebuild SHALL leave the prior derived
store usable — and SHALL be serialized so concurrent opens rebuild at most once.

#### Scenario: a fresh open rebuilds the Lance store from records

- **WHEN** `FindingsStore` opens against a `.kenn/findings/` directory of `<id>.json` records
- **THEN** the findings Lance store is rebuilt from those records
- **AND** each finding's embedding is reconciled from the findings sidecar by the fingerprint of its text

#### Scenario: a new finding's embedding reaches the sidecar, not a committed column

- **WHEN** a finding with previously unseen text is flushed
- **THEN** its vector is appended to the committed `.kenn/findings/vectors/` sidecar
- **AND** no embedding is written to a git-committed Lance column

#### Scenario: a crashed rebuild leaves the prior store intact

- **WHEN** a rebuild of the derived findings store is interrupted before it completes
- **THEN** the previously built derived store remains usable
- **AND** the next `open` redoes the rebuild

#### Scenario: concurrent opens rebuild at most once

- **WHEN** two processes `open` the findings store at the same time and both find the staleness gate fired
- **THEN** exactly one rebuild runs and the other reuses its result
