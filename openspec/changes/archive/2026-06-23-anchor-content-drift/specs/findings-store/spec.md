## MODIFIED Requirements

### Requirement: A finding's anchors and liveness are a mutable, mergeable sidecar

The store SHALL persist a finding's anchors and their liveness in a per-finding
append-only event log `.kenn/findings/<id>.anchor.jsonl`, git-tracked, with event
kinds `attach`, `rename`, and `detach` — separate from the immutable
record, because anchors point at files and directories that are moved, renamed,
and deleted and therefore SHALL NOT live in the append-only `.md`. An *anchor* is
a forward pointer from a finding to a place it applies. In v1 an anchor is a file
path or a directory subtree (a directory anchor matches every path beneath it);
symbol-level anchors are a future extension — code-node ids are themselves
unstable across reindex, and retrieval is by file/dir, so paths are the v1 grain.
A repeat `attach` to a path already in the set is the liveness signal; there is
no separate confirmation event. Each event SHALL
carry a timestamp and SHALL NOT carry a commit identifier, because events are
appended before the commit that contains them exists. An `attach` event MAY
additionally carry a content **sha** — an xxh64 hex digest of the anchored
**file** at attach time — so that read-time drift (a file whose content changed
since the finding was anchored) can be distinguished from a live or orphaned
anchor. The sha is optional: a directory anchor and an unreadable path carry
none, and an anchor log written before shas were recorded folds to `sha: None`,
which SHALL be treated as live (drift unknown), requiring no migration. The
current anchor set and per-anchor liveness SHALL be computed by folding the log:
recency is the latest `attach` timestamp for a path, relevancy is a
recency-weighted attach frequency, and the carried sha is the most-recent
`attach`'s sha — recent re-attaches weigh more and an anchor that stops being
re-attached decays — not a monotonic lifetime count. A `rename` SHALL carry the
prior sha to the new path (a pure move keeps the file's content, so its sha still
matches → live; a move-plus-edit no longer matches → drifted); a `detach` drops
it. The log SHALL be append-only
— a change is expressed as a new event, never an in-place edit — so that records
from different findings never conflict and concurrent appends to one finding's log
resolve as the union of lines. When a finding supersedes another (a correction),
the successor's anchor log SHALL be seeded with `attach` events for the
predecessor's current anchor set, carrying each anchor's sha, so a correction does
not reset reachability or drift state —
otherwise the superseding directive, preferred by retrieval, would have no anchors
and be unreachable by `find_directives`. Anchor events recorded for a finding at
creation SHALL follow that finding's flush/drop lifecycle — committed on flush,
discarded if the pending finding is dropped — so a dropped finding leaves no
orphan anchor log. Each append SHALL be atomic (a whole event line at a time).

#### Scenario: a correction inherits the superseded directive's anchors

- **GIVEN** a directive anchored to a file and a new finding that supersedes it
- **WHEN** the successor is created
- **THEN** the successor's anchor log is seeded with that anchor
- **AND** `find_directives` for that file returns the successor, not the original

#### Scenario: an anchor and its heartbeat fold from the log

- **WHEN** `<id>.anchor.jsonl` contains two `attach` events for the same file at
  different times
- **THEN** the folded state reports that file as a current anchor whose recency
  is the later timestamp and whose relevancy reflects both attaches

#### Scenario: a rename keeps the anchor resolvable

- **WHEN** a `rename` event records that an anchored file moved to a new path
- **THEN** the folded anchor set names the new path and not the old one

#### Scenario: re-attaching an existing anchor does not churn the record

- **WHEN** an `attach` event is appended for a path already in the anchor set
- **THEN** only `<id>.anchor.jsonl` changes and the `<id>.md` record is untouched

#### Scenario: an edited file's anchor folds to a drifted sha

- **WHEN** an `attach` event records a file's content sha
- **AND** the file is later edited so its content hash differs
- **THEN** read-time drift detection reports that anchor as drifted, not orphaned

#### Scenario: a sha-less anchor log is treated as live

- **WHEN** an anchor log contains `attach` events with no `sha` field
- **THEN** the folded anchors carry `sha: None`
- **AND** read-time drift detection never reports them as drifted
