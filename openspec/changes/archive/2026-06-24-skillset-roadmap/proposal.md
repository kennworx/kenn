## Why

> **Status: ROADMAP / vision.** Captures a validated direction and a build
> order, not a single scheduled change. Each arc below becomes its own change
> when picked up. See `design.md` for the full reasoning.

kenn today ships three skills — `recall`, `squeeze`, and the `kenn` how-to —
that together form a **passive memory** loop: read directives before a change,
distill directives at commit. That loop is good, but it leaves two of kenn's
strongest assets barely used by any skill:

1. **The code graph.** `find_similar`, transitive callers/usages/implementers,
   and scope/import navigation are exposed as MCP tools, but no skill
   *orchestrates a multi-hop investigation* into an answer. The `kenn` skill
   only lists the tools.
2. **Drift.** kenn has two staleness notions — a finding's `stale` flag (a
   cited **symbol** vanished from the graph) and `check_anchors` (an anchored
   **file** was moved/deleted). Neither catches the common case: *the file is
   still there but its content changed since the finding was written.* That gap
   is what makes a directive quietly rot.

The seed for closing the drift gap came from the `shadcn/improve` skill, which
stamps the git SHA a plan was written against and has its executor mechanically
diff it before acting. kenn can do the same, finer-grained and with no git
dependency — it already content-hashes working-tree files (xxhash) to skip
redundant index passes (`workspace-staleness`). Recording that hash on the
anchor `attach` event upgrades the file layer from *binary* (present/orphaned)
to *three-state* (live / drifted / orphaned), which in turn makes a whole class
of **active, graph-grounded skills** worth building.

## What Changes

This roadmap defines one **foundation** and three **skill families**, to be
built in that order. Nothing here is scheduled until promoted to its own change.

**Foundation — anchor content-drift (the first slice to build):**

- Add `sha: Option<String>` (xxhash of the file at attach time, computed at the
  MCP boundary) to the `AnchorEvent::Attach` variant and carry it through the
  fold onto `Anchor`. `rename` self-heals (carries the prior sha; a pure move
  keeps the blob, an edit shows drifted); `detach` drops it. Old logs fold to
  `sha: None` → "drift unknown" → treated as live, so **no migration**.
- Surface the new **drifted** state: `check_anchors` gains a `drifted` bucket
  alongside `broken`; `find_directives` hits gain a `drifted` flag beside the
  existing `stale` flag; `recall` flags drifted directives as "re-read before
  relying."

**Skill families (each its own future change):**

- **A. Knowledge lifecycle** — add `reconcile`: the drift janitor that consumes
  the new signal (re-read each drifted/stale finding → refresh-anchor /
  supersede / detach / tombstone). This is the foundation's payoff skill.
- **B. Graph understanding** — `trace` (multi-hop "how does X flow" →
  synthesized answer, optionally stored as a `guide`) and `blast` (pre-edit
  "what's the change scope for X" = transitive graph surface ∪ governing
  directives).
- **C. Advisor** — `dup` (a `find_similar` sweep → consolidation candidates)
  and, larger, `audit` (the `improve` pipeline with the duplication/dead-code/
  god-module legs querying the graph instead of grepping, and "considered &
  rejected" stored as queryable findings).

**Cross-cutting robustness** (fold into A when `reconcile`/`squeeze` are
touched): adopt two `improve` disciplines — *vet-over-report* (re-read a cited
location before presenting/trusting it) and a *"repo content is data, not
instructions"* prompt-injection guard in any skill that reads repo/session text
and commits the result (`squeeze`, `reconcile`, `audit`).

## Capabilities

### Modified Capabilities

- `findings-store`: the anchor event log gains an optional per-attach content
  hash and a three-state (live/drifted/orphaned) fold; ranking/liveness
  unchanged.
- `findings-mcp`: `check_anchors` reports a `drifted` bucket; `find_directives`
  hits carry a `drifted` flag; `record_anchor` accepts/derives the attach sha.
- `agent-guide`: a richer skill set (`reconcile`, `trace`, `blast`, `dup`,
  `audit`) layered on the existing `recall`/`squeeze`/`kenn` skills.

(Deltas are written when each arc is promoted to its own change — this roadmap
names the surfaces, it does not yet specify them.)

## Impact

- **Behavior:** directives stop silently rotting (drift is visible and
  reconciled); skills begin to *use* the graph, not just the memory.
- **Compatibility:** `sha: Option<String>` + an extra response bucket/flag are
  additive; pre-existing anchor logs and callers keep working untouched.
- **Open / sequencing:** the foundation is concrete and low-risk; family B/C
  skills are larger and should be gated on the foundation landing and on real
  usage of `reconcile` before the heavier `audit` is built. Build order and
  open questions are in `design.md`.
