## Why

kenn exposes good code-graph tools and one `SKILL.md`, but it has no way to
carry the **user's steering** — the directions and corrections a user gives
during work — forward into later sessions, correlated to the code those
directions are about. That knowledge today lands in three homes, and each misses
a cell:

```
                 audience      load             team-shared?
  CLAUDE.md      Claude only   always (∴ small) yes (in repo)
  feedback_*.md  Claude only   on recall        NO (~/.claude, local machine)
  kenn findings  ALL agents    on relevance     YES (committed)
```

The gap is **team-shared, agent-agnostic, relevance-loaded, code-correlated
directives, surfaced at the point of work** — the "case law" of a repo, distinct
from CLAUDE.md's small always-on "constitution" and from a developer's private,
machine-local notes. kenn is the only home that is simultaneously committed,
agent-agnostic, and relevance-loaded, so directives belong there.

Governing principle: **kenn is not intelligent.** It stores knowledge,
retrieves it, and guides the agent on how to access and combine it. The agent
(Claude, etc.) performs the intelligence — classifying directions, distilling
them, judging relevance, asking the user. This change adds the substrate and the
guidance, **not** reasoning inside kenn.

## What Changes

- **Directive/guide = a finding + a tag.** No new record kind: `tag:directive`
  (a do/don't rule) or `tag:guide` (how-to / orientation knowledge). Pure
  convention over the existing findings store.
- **Finding record format `.json` → `.md` with YAML frontmatter.** The record
  becomes human- and agent-readable without a tool: immutable frontmatter
  (`id`, `tags`, `parent_ids`, `created_at`) plus a prose body. The body is the
  embedding source (value lives in the prose, not the structure).
- **Anchors + liveness move out of the immutable record into a mutable,
  mergeable sidecar.** An *anchor* is a forward pointer from a finding to a place
  it applies (a file or dir-subtree path in v1; symbol anchors deferred — node
  ids are themselves unstable and retrieval is by file/dir). Because files/dirs
  get moved,
  renamed, and deleted, anchors cannot live in an append-only record. They live
  in a per-finding append-only event log `.kenn/findings/<id>.anchor.jsonl`
  (`attach` / `rename` / `detach`; a repeat `attach` to a path already in the set
  is the liveness signal — there is no separate "fire" event), folded to the
  current anchor set + recency-weighted liveness. Events carry a `ts` only (no
  commit hash — events are appended before the commit exists). `parent_ids`
  (backward provenance) stay immutable in the md.
- **New MCP tools** (mechanical, no reasoning):
  - `find_directives(paths)` — retrieve directives/guides relevant to a set of
    files/dirs, ranking by RRF of anchor/dir-prefix match ⊕ body-vector
    proximity, boosted by liveness. Degrades to the anchor leg alone when the
    embedder/index is not ready (anchors are committed files, resolvable without
    an index).
  - `check_anchors` — report anchors that no longer resolve, so the agent can
    apply renames/deletes before commit.
  - record an anchor event (`attach` / `rename` / `detach`); `store_finding`
    gains an optional `anchors` list so a directive is created and anchored in
    one call.
- **Orientation by snapshot file with a tool fallback.** `kenn index` writes a
  snapshot-local `overview.md`; the agent reads it directly. Absent file =
  "no current snapshot" → the guide names the status/overview tool to call. No
  MCP resources (their indexing-error UX is client-dependent and unreliable).
- **The directive workflow extends the existing portable fragment.** kenn already
  ships an orchestrator-independent system-prompt fragment that drives finding
  accumulation (findings-mcp). The recall/before-commit ritual extends *that*
  fragment — it stays orchestrator-independent — rather than being reinvented per
  agent. The before-commit ritual is **guidance, not a git hook**:
  `check_anchors → pull by file/dir → re-attach the relevant ones → squeeze`.
- **The "collect" layer already exists — reuse it.** `conversation-history-store`
  already captures, per project and branch, which files a session touched, the
  user's prompts, and the `transcript_path` (machine-local `collector.db`, no
  LLM). The squeeze *reads* this as its source for directions and file activity
  (anchoring directives to the staged diff) — it adds **no new capture
  mechanism**. `conversation-history-store` is a dependency, not modified.
- **An agent-guide layer = the Claude-Code surfacing of that workflow.** A
  refocused plugin guide that routes the agent across tools, the run-local
  orientation file, and skills, plus `squeeze` / `recall` skills that package the
  fragment's workflow for Claude Code (the fragment remains the source of truth).

## Capabilities

### Added Capabilities

- `agent-guide`: the Claude-Code surfacing — plugin guide (router), `squeeze` /
  `recall` skills packaging the portable workflow, and the run-local
  `overview.md` orientation pattern (file + tool fallback).

### Modified Capabilities

- `findings-store`: finding record format becomes md + frontmatter; the
  directive/guide tag convention; anchors + liveness as a mutable, mergeable
  per-finding sidecar; `store_finding` gains optional `anchors`; supersede seeds
  the successor's anchors.
- `findings-mcp`: adds `find_directives`, `check_anchors`, and anchor-event
  recording; extends the orchestrator-independent system-prompt fragment to drive
  the recall/before-commit directive workflow. Consistent with the existing "runs
  no model / no task analysis" requirement — the new tools are primitive.
- `store-layout`: committed root holds `findings/<id>.md` and
  `findings/<id>.anchor.jsonl`; the active index run holds `overview.md`.

## Impact

- **Storage migration:** existing `findings/<id>.json` records convert to
  `<id>.md`. Trivial today (one record) and consistent with the project's
  "change shapes in place while prototyping" stance — no version gate.
- **Behavior:** agents gain code-correlated, team-shared directives surfaced
  where they apply, plus a guided before-commit capture-and-guardrail ritual —
  when the agent runs it, new code that contradicts a past directive can be
  caught at commit time (the ritual is guidance, not an enforced hook). Default
  findings/search behavior is preserved.
- **Boundary preserved:** kenn remains a dumb durable store + a few path tools +
  guidance; all classification, distillation, and judgement stay in the agent.
- **Team sharing:** all new state is committed md + jsonl, merge-clean by
  construction (per-finding files; mutation expressed as appended events).
- **Safety:** because directives are committed (unlike the machine-local memory
  this automates), the squeeze applies a redaction gate — no credentials,
  machine-local paths, or private project/customer identifiers in committed
  directives; unshareable steering stays in machine-local personal notes.
