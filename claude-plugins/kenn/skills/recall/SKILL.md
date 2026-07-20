---
name: recall
description: Surface kenn directives and guides relevant to the file(s)/dir(s) you are about to work on, so you start informed. Use when beginning work on an area of the codebase, or when the user says "recall", "what do we know here", "any directives for this", or "anything I should know before editing X".
argument-hint: "[file or dir paths — defaults to the files you're about to change]"
user-invocable: true
---

Surface the durable steering that applies to the code about to be touched. This
is the start-of-work counterpart to `squeeze` (which captures at commit time).

Paths to recall for: `$ARGUMENTS` if given, otherwise the file(s)/dir(s) you are
about to edit.

Steps:

1. Run `kenn findings directives <paths…>` (add `--query <q>` to bias the
   semantic leg toward what you're about to do). It returns findings tagged
   `directive` (rules) and `guide` (orientation/context) that are anchored to —
   or semantically near — those paths, ranked by anchor match and liveness. It
   works even before the index is warm (it falls back to the anchor leg alone).
2. Present what came back so you start informed:
   - **Directives** grouped by polarity — `polarity:dont` (do-not rules you must
     not violate in the change you're about to make) first, then `polarity:do`.
   - **Guides** separately, as context (how-to / orientation — not rules).
   - For each, show its text and the files/dirs it's anchored to. Flag any
     marked `stale` (their cited code evidence moved) or `drifted` (a file the
     directive is anchored to changed content since it was written) — **re-read
     a stale/drifted directive against the current code before relying on it**,
     since its ground truth may have shifted.
3. **Do not invent directives.** Present only what `kenn findings directives`
   returned. If it returns nothing, say so plainly ("no recorded directives for
   these paths") — do not fabricate guidance.

If `kenn` isn't on `PATH`, install the binary; if it reports no index, run
`kenn index` once to build it, then re-run. (The same lookup exists as the
`find_directives` MCP tool if that plugin happens to be loaded.)
