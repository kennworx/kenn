## ADDED Requirements

### Requirement: Anchor liveness records per-file content drift

An `attach` event SHALL be allowed to carry the content hash of the anchored
file at attach time — the xxhash digest the workspace-staleness gate already
uses — supplied by the caller so the store stays clock-free and reproducible;
the store SHALL NOT read the filesystem to compute it. The fold SHALL carry the hash onto the
current anchor alongside recency and attach-count, taking it from the latest
`attach`, and `rename` SHALL carry the prior hash to the new path. From the
hash, a file anchor SHALL resolve to one of three states: **live** (path exists
and the current file hash matches the recorded hash), **drifted** (path exists
but the hash differs — the file changed since the finding was written), or
**orphaned** (path no longer resolves). An `attach` with no recorded hash (every
pre-existing log) SHALL fold to "drift unknown" and be treated as live, so no
migration is required. Directory anchors carry no hash in v1 and remain
exists-only. The drift state SHALL NOT change anchor ranking or recency-weighted
liveness — it is a separate freshness signal, distinct from the symbol-level
`stale` flag (which tracks whether a finding's code-node `parent_ids` resolve).

#### Scenario: attach records a content hash and a matching file reads live

- **GIVEN** a finding attached to a file with the file's content hash recorded
- **WHEN** the anchor is folded and the file is unchanged
- **THEN** the anchor resolves to the **live** state

#### Scenario: an edited file reads drifted

- **GIVEN** a finding whose anchor recorded hash `H` for a file
- **WHEN** the file's current content hash is not `H` and the path still exists
- **THEN** the anchor resolves to the **drifted** state

#### Scenario: a pure rename stays live, a rename-plus-edit drifts

- **GIVEN** an anchor with recorded hash `H` that is renamed via `rename`
- **WHEN** the file content at the new path still hashes to `H`
- **THEN** the anchor resolves to **live**; if the content was also edited, it
  resolves to **drifted**

#### Scenario: a legacy attach without a hash is treated as live

- **GIVEN** an `attach` event with no recorded content hash
- **WHEN** the anchor is folded
- **THEN** drift is unknown and the anchor is treated as **live** (no migration)
