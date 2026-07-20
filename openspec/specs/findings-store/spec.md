# findings-store Specification

## Purpose
TBD - created by archiving change findings-backend. Update Purpose after archive.
## Requirements
### Requirement: A finding is a durable, provenance-bearing record

The store SHALL persist a finding as a committed per-finding record file —
`.kenn/findings/<id>.md` — whose immutable YAML frontmatter carries a unique
`id`, a list of free-form `tags`, a list of `parent_ids`, and a `created_at`
timestamp, and whose markdown body is the finding's free-form prose `text`. The
body SHALL be the embedding source (the value lives in the prose, not the
frontmatter). `parent_ids` SHALL be drawn from a single ID space shared with
code-graph nodes, so a parent MAY be a code-graph node or another finding, and
SHALL be immutable: a finding is corrected only by creating a new finding that
supersedes it, never by editing the record in place. The finding's embedding
SHALL NOT be part of the record — it is derived (see the requirement "the
findings derived store and embeddings are derived").

The record frontmatter SHALL NOT carry the finding's anchors or liveness; those
are mutable and live in a separate sidecar (see "A finding's anchors and liveness
are a mutable, mergeable sidecar"). The store SHALL NOT impose a fixed `kind`
enumeration on findings; classification is carried by `tags`.

#### Scenario: a finding round-trips with its provenance

- **WHEN** a finding is stored with text, tags, and `parent_ids` referencing a
  code-graph node and an earlier finding
- **THEN** retrieving it by `id` returns the same text, tags, and `parent_ids`

#### Scenario: a finding record is a committed markdown file

- **WHEN** a finding is flushed
- **THEN** a `.kenn/findings/<id>.md` file holds `id`, `tags`, `parent_ids`, and
  `created_at` in frontmatter and the `text` as its body
- **AND** that file is git-tracked, while the findings derived store is not

#### Scenario: the body is the embedding source

- **WHEN** a finding's embedding is produced
- **THEN** it is computed from the markdown body, not the frontmatter

### Requirement: the findings derived store and embeddings are derived

The findings derived store SHALL be a SQLite database, rebuilt from the committed `<id>.md`
records, and gitignored, never committed. A finding's embedding SHALL be stored in a dedicated
findings vector sidecar (`.kenn/vectors/findings/`) that reuses the `incremental-embedding`
sidecar format, keyed by the fingerprint of its body (the prose `text`), and reconciled into the
rebuilt store on open; it SHALL NOT be persisted in a committed embedding column. The findings
sidecar SHALL be separate from the code sidecar — they have independent compaction live-sets and
independent manifests. The findings store SHALL use no storage engine other than SQLite.

The rebuild SHALL be atomic — a crash mid-rebuild SHALL leave the prior derived store usable —
and SHALL be serialized so concurrent opens rebuild at most once.

#### Scenario: a fresh open rebuilds the SQLite store from records

- **WHEN** `FindingsStore` opens against a `.kenn/findings/` directory of `<id>.md` records
- **THEN** the findings SQLite store is rebuilt from those records
- **AND** each finding's embedding is reconciled from the findings sidecar by the fingerprint of its body

#### Scenario: a new finding's embedding reaches the sidecar, not a committed column

- **WHEN** a finding with previously unseen body text is flushed
- **THEN** its vector is appended to the committed `.kenn/vectors/findings/` sidecar
- **AND** no embedding is written to a git-committed embedding column

#### Scenario: a crashed rebuild leaves the prior store intact

- **WHEN** a rebuild of the derived findings store is interrupted before it completes
- **THEN** the previously built derived store remains usable
- **AND** the next `open` redoes the rebuild

#### Scenario: concurrent opens rebuild at most once

- **WHEN** two processes `open` the findings store at the same time and both find the staleness gate fired
- **THEN** exactly one rebuild runs and the other reuses its result

### Requirement: store_finding persists a finding and reports near-duplicates

`store_finding` SHALL accept `text`, `parent_ids`, `tags`, and an optional
`anchors` list (file or directory paths), persist the finding, return its `id`,
and additionally return any existing findings whose content is semantically
similar above a threshold. When `anchors` are supplied, the store SHALL record an
initial `attach` event for each in the new finding's anchor log. The store SHALL
NOT auto-merge or auto-discard on similarity — it SHALL return the matches and
leave the decision to the caller.

#### Scenario: a similar prior finding is surfaced

- **GIVEN** a finding semantically close to one already stored
- **WHEN** `store_finding` is called
- **THEN** it returns the new finding's `id`
- **AND** it returns the similar prior finding among its results

#### Scenario: supplied anchors are recorded on create

- **WHEN** `store_finding` is called with an `anchors` list
- **THEN** the new finding's anchor log records an `attach` for each anchor

### Requirement: findings are searchable by hybrid lexical + vector query

`search_findings` SHALL return findings ranked by a combination of BM25 over `text` and vector similarity over `embedding`. Results SHALL be deterministic for a fixed query and corpus.

The search SHALL be served from a **persistent** index built outside the read path — created from the committed records when the store opens and maintained as findings are written — NOT a transient index built per call. The read path MUST NOT create a table or build an index. The lexical stage SHALL push its limit into the persistent index query so the candidate set is capped, and the result set SHALL be resolved to full records only for the top-`limit` hits (no full-corpus record load). Lifecycle (superseded / tombstoned) SHALL be filtered within the index query.

#### Scenario: a finding is retrieved by meaning

- **WHEN** `search_findings` is called with a query that paraphrases a stored finding without sharing exact terms
- **THEN** that finding appears in the ranked results

#### Scenario: no index is built on the read path

- **WHEN** `search_findings` is called
- **THEN** it queries the persistent findings index
- **AND** it does NOT create a table or build an index, and resolves only the top-`limit` records rather than loading every finding

### Requirement: the derivation DAG is traversable

The store SHALL expose `find_predecessors` and `find_successors` over a finding, walking `parent_ids` edges. Because a finding may only reference earlier-created findings, the derivation graph SHALL be acyclic.

#### Scenario: provenance traces to source evidence

- **GIVEN** a finding whose parents include another finding that in turn references a code-graph node
- **WHEN** `find_predecessors` is walked transitively
- **THEN** the walk reaches the originating code-graph node
- **AND** the walk terminates (no cycle)

### Requirement: findings are append-only; corrections supersede and deletions tombstone

The store SHALL NOT modify a finding in place. A correction SHALL be a new finding carrying the prior finding in `parent_ids` and a `supersedes` tag; retrieval SHALL prefer the latest finding in a supersede chain. A deletion SHALL be a tombstone finding referencing the target; retrieval SHALL exclude tombstoned findings from normal results.

Because every finding file is write-once and uniquely named, a `git merge` of two branches that each added findings SHALL union them with no conflict.

#### Scenario: a correction supersedes without mutating

- **WHEN** a finding is corrected by storing a new finding that supersedes it
- **THEN** the original finding is still retrievable by `id`
- **AND** a default `search_findings` returns the superseding finding, not the original

#### Scenario: two branches add findings and merge cleanly

- **GIVEN** branch A and branch B each stored new findings
- **WHEN** branch B is merged into branch A with a plain `git merge`
- **THEN** the merge completes with no conflict
- **AND** the merged store contains the findings from both branches

### Requirement: staleness is computed at read time

The store SHALL NOT persist a staleness flag on a finding. At query time, the store SHALL check whether a finding's code-graph `parent_ids` still resolve in the current branch's code graph; if any do not, the result SHALL be flagged stale. A stale finding SHALL still be returned, marked, not omitted. The resolution SHALL key on the **canonical code-node id** — the `pub_id` as returned by `find_symbol` and stored in a finding's `parent_ids` (e.g. `rs:foo::bar`, `cs:Ns.Type`), which already carries the language short-code. The resolver SHALL NOT re-prefix it with the `language` column (`rust`/`csharp`/…); doing so doubles the id (`rust:rs:foo`) so it never matches and every code-cited finding falsely folds to stale.

#### Scenario: a finding over removed code is flagged, not deleted

- **GIVEN** a finding whose evidence is a code-graph node
- **WHEN** the code graph is rebuilt on a branch where that node no longer exists, and the finding is queried
- **THEN** the finding is returned with a stale flag

#### Scenario: the same finding is live on a branch where the code remains

- **WHEN** the same finding is queried on a branch where its evidence node still exists
- **THEN** the finding is returned without a stale flag

#### Scenario: a finding citing a present symbol by its canonical id is live

- **GIVEN** a finding whose `parent_id` is a symbol's canonical id (e.g. `cs:Ns.Type`) that exists in the current code graph
- **WHEN** the finding is queried
- **THEN** it is returned without a stale flag, because the resolver keys on the same canonical `pub_id`, not a language-doubled form

### Requirement: findings reach the committed store via an explicit flush

`store_finding` SHALL write to a pending area. Findings SHALL enter the committed store on an explicit flush. The default flush policy SHALL commit every pending finding the caller has not explicitly dropped.

#### Scenario: pending findings are committed on flush

- **WHEN** several findings are stored and then a flush is invoked without dropping any
- **THEN** all of them appear in the committed store
- **AND** they are present after a fresh open of the store

#### Scenario: a dropped finding is not committed

- **WHEN** a pending finding is explicitly dropped before flush
- **THEN** it does not appear in the committed store

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

### Requirement: Directives and guides are findings distinguished by tag

The store SHALL treat a *directive* and a *guide* as ordinary findings carrying a
reserved tag — with no separate record kind — and the two tags carry distinct
roles. A `directive` is a **rule** (a do/don't): it carries a `polarity:*` tag
(`polarity:do` / `polarity:dont`) and is the subject of the before-commit
violation check. A `guide` is **orientation / how-to context**: it is retrievable
alongside directives but is not a rule and is not violation-checked. Retrieval
SHALL be able to filter findings to these tags.

#### Scenario: a directive is a polarity-bearing rule

- **WHEN** a finding is stored with `tag:directive` and `tag:polarity:dont`
- **THEN** it round-trips as a normal finding
- **AND** it can be retrieved filtered to `tag:directive`

#### Scenario: a guide is retrievable context, not a rule

- **WHEN** a finding is stored with `tag:guide`
- **THEN** it is retrievable alongside directives for a path
- **AND** it carries no polarity and is not treated as a violation-checkable rule

