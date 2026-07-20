---
id: fnd_febe5aba-b404-4431-a053-412fb6d6fbec
tags:
- gotcha
- dogfood-2026-06-24
parent_ids:
- rs:kenn-store::staleness::compute_staleness_key
created_at: 2026-06-24T16:28:42.347457Z
---
GOTCHA (fixed): compute_staleness_key must NOT drop a dirty tracked file whose read fails — that's a DELETION, and git status reports it. The prior code did `fs::read(abs).ok()?` inside a filter_map, so a deleted file silently vanished from the dirty-file set, leaving the git staleness key equal to the clean pre-delete state → `kenn index` reported "staleness key unchanged" and SKIPPED the reindex, so deleted symbols stayed in the graph. Fix: map a deleted/unreadable dirty file to a deletion sentinel (u64::MAX) via a `dirty_entry` helper so the deletion changes the key. Edits were always caught (content hash changes); only deletions slipped. Found dogfooding on a large external repo (deleting a .cs file did not trigger a reindex; needed --force).