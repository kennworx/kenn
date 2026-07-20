## Why

Surfaced while dogfooding on a large repo: after deleting a tracked `.cs` file,
`kenn index` reported "staleness key unchanged" and **skipped the reindex** — the
deleted symbol stayed in the graph. Root cause: `compute_staleness_key`
(`crates/kenn-store/src/staleness.rs`) builds the git key's dirty-file set with
`std::fs::read(abs).ok()?` — for a **deleted** file the read fails, so the entry
is silently dropped. `git status` *does* report the deletion, but dropping it
leaves the dirty set identical to the clean pre-delete state (where the file was
not dirty at all), so the key matches and the reindex is skipped.

Effect: deleting a tracked source file does not trigger a reindex; the index lags
behind the deletion until some other change (or a commit advancing `HEAD`, or
`--force`) moves the key. Edits are caught (their hash changes); deletions are not.

## What Changes

- A dirty tracked file that is absent/unreadable (a deletion) contributes a
  **deletion sentinel** entry to the git key instead of being dropped, so the
  deletion changes the key and the reindex fires. Readable files still hash their
  content as before. Factored into a small `dirty_entry` helper.
- Regression tests: a deleted tracked file changes the key (and is represented by
  the sentinel); the helper hashes a present file and sentinels a missing one.

## Capabilities

### Modified Capabilities

- `workspace-staleness`: the git key SHALL register a tracked **deletion** (a
  sentinel entry), not drop it.

## Impact

- **Bugfix** — staleness only; the change can only cause *more* reindexes (never
  fewer), the always-safe direction. No schema or API change.
