## 1. Store — anchor event carries a content sha

- [x] 1.1 Add `sha: Option<String>` to `AnchorEvent::Attach` and to the folded
  `Anchor` struct (`crates/kenn-store/src/db/findings/anchor.rs`). → verify:
  serde round-trips with and without the field (old logs → `None`).
- [x] 1.2 Carry sha through `fold`: Attach sets the most-recent-attach sha,
  Rename carries the prior sha to the new path, Detach drops it. → verify: fold
  unit tests assert sha on attach, carry on rename, drop on detach.
- [x] 1.3 `seed_from_predecessor` carries each predecessor anchor's sha into the
  seeded Attach. → verify: a superseding finding inherits the sha.
- [x] 1.4 Add a public `file_content_sha(path) -> Option<String>` helper
  (xxh64 hex, `None` for unreadable/dir), reused by the boundary and readers. →
  verify: matches the `xxh64(&bytes, 0)` staleness format.

## 2. Store — read-time drift

- [x] 2.1 `check_anchors` returns broken **and** drifted buckets: an anchor that
  exists but whose file hash differs from the recorded sha is drifted (missing =
  broken, not drifted; `sha: None` or a dir = live). → verify: store test seeds
  attach-with-sha, edits the file, sees drifted; unchanged file is live.
- [x] 2.2 `find_directives` hits carry a `drifted` flag (any file anchor of the
  finding drifted). `FindingHit` gains `drifted: bool` (false on the
  search/semantic paths). → verify: a directive whose anchored file changed
  surfaces with `drifted: true`.

## 3. MCP — boundary sha + response surface

- [x] 3.1 `store_finding` and `record_anchor` compute the sha at attach from
  `state.source_root().join(anchor)` and pass it into `AnchorEvent::Attach`
  (`crates/kenn-mcp/src/tools/{findings,anchors}.rs`). → verify: an attached file
  anchor records a sha; a dir anchor records `None`.
- [x] 3.2 `CheckAnchorsResponse` gains a `drifted` field; `FindingView` gains a
  `drifted: bool` (serde default); `finding_to_view` threads it. → verify: tool
  responses expose drifted.

## 4. Skills

- [x] 4.1 `recall` surfaces drifted directives ("re-read before relying");
  `squeeze` step 0 reports drift. → verify: skill docs reference the drifted flag.

## 5. Verification

- [x] 5.1 No migration: an anchor log written before this change folds to
  `sha: None` and is treated as live (never drifted). → verify: a sha-less log
  yields no drift.
- [x] 5.2 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
  `cargo fmt --all` last.
