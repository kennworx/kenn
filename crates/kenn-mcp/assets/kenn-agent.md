# Using kenn

kenn exposes, over MCP tools, an **indexed code graph** plus a **findings &
directives** knowledge layer over the workspace.

## Navigating the code graph

Read-only code structure over an indexed workspace.

- Start with `get_index_status` to verify the index is available, then
  `get_workspace_overview` for orientation. If the index is still building
  (`state: "indexing"`, or a tool returns `INDEX_UNAVAILABLE`), call
  `wait_for_index` to block until it is ready instead of treating an early
  empty result as final.
- Symbol search: `find_symbol(name)` for literal lookups (returns `match_kind`
  per row); `search_symbols(query)` for natural-language intent (BM25 over name
  + docs, blended ranking, returns a `score` per row).
- `find_at_location` for stack-trace lookups.
- Navigate the graph with `list_callers` / `list_callees` / `list_usages`.
- Cursor pagination: pass the returned `next` cursor verbatim; a `STALE_CURSOR`
  error means the index rotated — restart.

## Knowledge layer — the findings store

You have a **findings store** — a durable, shared memory of conclusions,
reachable through MCP tools. It carries knowledge across tasks and sessions. Two
habits make it pay off:

### Search before you re-investigate

Before digging into a question, call `search_findings` with a short query
describing it. A prior conclusion may already be recorded — read it instead of
re-deriving it. Each hit carries a `stale` flag: when `stale` is true, the
finding's code evidence has changed since it was written, so verify before
relying on it. `semantic_search` with `scope: "both"` searches code and findings
together when you are orienting in unfamiliar territory.

### Store at a stable conclusion

When you reach a **stable conclusion** — a verified fact, a decision, a plan, a
non-obvious gotcha — call `store_finding`. Do **not** store after every
intermediate thought; store the durable result, not the search for it.

- `text` — the conclusion, stated plainly enough to be useful months later, out
  of this conversation's context.
- `parent_ids` — the evidence: code-node ids (`<lang>:<pub_id>`) and/or earlier
  finding ids (`fnd_…`) this conclusion derives from. Provenance is what lets a
  later reader ask "why?" and `find_predecessors` answer.
- `tags` — free strings, no enforced vocabulary. A useful starter set: `evidence`
  (a verified observation), `gotcha` (a non-obvious trap), `plan` (an intended
  course of action), `decision` (a settled choice).

### Lifecycle: corrections and deletions

Findings are append-only — never edited in place.

- **Correcting** a finding: store the corrected finding with tag
  `supersedes:<old_id>` and `<old_id>` in `parent_ids`. The old finding drops out
  of `search_findings` results; its record is still readable via `get_finding`.
- **Retracting** a finding: store a tombstone finding with tag
  `tombstone:<target_id>` and `<target_id>` in `parent_ids`.
- **Synthesis:** when several findings combine into a higher-level conclusion,
  use `merge_findings` — it records the inputs as `parent_ids` and keeps the
  originals as evidence.

## Directives — code-anchored steering

A **directive** is a finding tagged `directive` carrying a `polarity:do` /
`polarity:dont` rule; a **guide** is a finding tagged `guide` holding
orientation / how-to context. Both are *anchored* to the files/dirs they apply
to, so they resurface exactly where they matter. This turns the user's steering
into durable, team-shared rules the next agent sees.

### Recall before you work on an area

Before editing an area, call `find_directives` with the file(s)/dir(s) you are
about to touch. It returns the directives (rules) and guides (context) anchored
to — or semantically near — those paths, liveness-ranked, excluding
superseded/tombstoned, each with a `stale` flag. It works before the index is
warm (anchor-only). Heed `polarity:dont` rules; treat guides as context.

### Capture before you commit

Run this ritual before a commit — it is guidance, not an automated hook:

1. **Repair anchors.** Call `check_anchors`; for each unresolved anchor, if your
   diff renamed the file `record_anchor` a `rename`, if it deleted the file a
   `detach`.
2. **Pull + guardrail.** Call `find_directives` for the staged diff's files/dirs.
   Warn if the diff violates a `polarity:dont` directive (your judgement — guides
   are never violation-checked).
3. **Re-attach what applied.** For each directive that genuinely applied to the
   change, `record_anchor` an `attach` for the changed path (the liveness
   signal). Attach on confirmed relevance, not for everything surfaced.
4. **Distill new rules.** The directions and corrections to distill are in this
   conversation — re-read the session for the user's instructions and the points
   where they corrected course. (kenn's `cc-hook` also persists prompts and
   touched files per branch to a machine-local `collector.db` for later/other
   agents; there is no read tool for it yet, so rely on the live conversation
   here.) Favor corrections and recurring instructions over one-off praise.
   Create each with `store_finding` (its `anchors` field anchors in one call):
   `text` = the rule, `tags` = `["directive","polarity:do"|"polarity:dont"]` (or
   `["guide"]`), `anchors` = the files/dirs it governs. Supersede when a rule
   changed.

### Where a rule belongs (routing)

- Small, universal, must-apply-every-turn → the agent's always-on instructions
  (e.g. a project `CLAUDE.md`), **not** a directive.
- A developer's private, machine-local working style → a machine-local personal
  note, **not** the shared store.
- Team-shareable and specific to this code → a **kenn directive**, anchored.

### Redaction (directives are committed and shared)

Before writing a committed directive, never include credentials, machine-local
absolute paths, or private project/customer identifiers. Route anything
unshareable to a machine-local personal note instead. When the intent or the
right home is unclear, ask rather than commit.
