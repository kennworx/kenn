## Why

> Promotes the **foundation slice** of the `skillset-roadmap` to its own change.
> See that roadmap's `design.md` for the full reasoning.

kenn has two staleness notions: a finding's `stale` flag (a cited **symbol**
vanished from the graph) and `check_anchors` (an anchored **file** was
moved/deleted). Neither catches the common case that quietly rots a directive:
*the file is still there but its content changed since the finding was written.*

The file layer is **binary** today — an anchor is either present or orphaned.
kenn already content-hashes working-tree files (xxhash) for index staleness, so
recording that hash on the anchor `attach` event upgrades the file layer to
**three-state**: live / **drifted** / orphaned. That drift signal is the
precondition for the active, graph-grounded skills the roadmap defines (`reconcile`
especially), and it immediately sharpens the two skills already in daily use —
`recall` and `squeeze`.

## What Changes

- Add `sha: Option<String>` (xxh64 hex of the file at attach time, computed at
  the MCP boundary from the live working tree) to `AnchorEvent::Attach`, carried
  through the fold onto `Anchor`. `rename` self-heals — the fold carries the
  prior sha to the new path, so a pure move keeps the blob (same sha → live) and
  a move-plus-edit shows drifted. `detach` drops it. A `supersede` seeds the
  successor with the predecessor's sha.
- **No migration.** Old logs have no `sha` field → folds to `None` → "drift
  unknown" → treated as live. The field is `#[serde(default)]`-compatible.
- Surface the **drifted** state:
  - `check_anchors` returns a `drifted` bucket alongside `broken`.
  - `find_directives` hits carry a `drifted` flag beside the existing `stale`.
  - `recall` flags drifted directives as "re-read before relying"; `squeeze`
    step 0 reports drift on the directives it is about to reconcile against.

## Capabilities

### Modified Capabilities

- `findings-store`: the anchor event log records a content sha at attach; the
  fold carries it; read-time drift is derived from it.
- `findings-mcp`: `check_anchors` adds a drifted bucket; `find_directives` hits
  carry a drifted flag; `attach` computes the sha at the boundary.

## Impact

- **No migration, no schema change** — anchors are a JSONL event log; the new
  field is additive and optional.
- **Read-time only** — drift is computed by hashing the live file at read time,
  never persisted as state; consistent with how `stale` works.
- **Skills** — `recall` and `squeeze` markdown gain a drift line.
- **Scope** — `kenn-store` (anchor fold + readers), `kenn-mcp` (boundary sha +
  response fields), two skill docs.
