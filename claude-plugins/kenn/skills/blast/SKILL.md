---
name: blast
description: Pre-edit blast radius — given a symbol or file you're about to change, walk the call/type/usage graph to the transitive change surface and pull the directives that govern the touched files. Use before editing, or when the user asks "what will this affect", "what's the blast radius", "what calls/uses this", "what's the change scope for X", or "is it safe to change X".
argument-hint: "[symbol name, file:line, or path you're about to change]"
user-invocable: true
---

The pre-edit counterpart to `recall`, with the graph dimension added. `recall`
answers "what steering governs these files?"; `blast` also answers "**what will
this change touch?**" — the transitive set of callers, usages, and implementers a
change ripples into — and fuses the two: **change surface ∪ governing memory**.
Read-only: it orients before an edit, it does not write anything. Judgement is
yours; kenn supplies the graph and the directives.

Drive kenn through the **`kenn` CLI** (run commands with the Bash tool). Every
command prints **TOON** by default; add `--json` where you parse output.

Target: `$ARGUMENTS` if given (a symbol name, a `file:line`, or a path),
otherwise the symbol/file you are about to edit.

Run these steps:

**1. Resolve the target to a symbol id.** Use `kenn find usages <query>` (it
takes a name / path / `pub_id` and fuses lookup + references in one call), or
`kenn find symbol <name>` / `kenn find at-location <file> <line>` for a
`file:line`. If the name is ambiguous, narrow by `--kind`/`--path`/`--package`
and say which target you picked.

**2. Walk the graph to the change surface.** From the resolved id, gather the
incoming edges that a change ripples into:
- `kenn list callers <id>` — who calls it.
- `kenn list usages <id>` — every reference (tagged by edge kind).
- `kenn list implementers <id>` / `kenn list overrides <id>` — for an
  interface/trait/method, the concrete impls and overrides that must move in
  lockstep.

These graph-walk commands **exclude test-file symbols by default**. For a blast
radius you want the *whole* change surface, so pass `--include-tests` — test
callers must move in lockstep too.

Walk **transitively** a couple of hops out (callers-of-callers) where it matters —
a signature change propagates past the immediate ring. Dedup by symbol id, and
collect the **set of files** the surface touches.

> **Bound the walk and say so.** Pick a hop limit (e.g. 2–3) and a per-node
> fan-out you'll follow. If a node has a large fan-in (a hot function with
> hundreds of callers) or the surface keeps growing, **stop and report the
> truncation** — "walked 2 hops; `foo` has 200+ callers, not expanded" — never
> present a silently capped set as the complete blast radius.

**3. Pull the governing directives.** Run `kenn findings directives <paths…>`
with the collected files (and the target's own file/dir). These are the rules
that govern what you're about to touch.

**4. Present surface ∪ memory.** Two sections:
- **Change surface** — the affected symbols grouped by relation (callers /
  usages / implementers / overrides), with their files; note the walk bound and
  any truncation.
- **Governing directives** — `directive` hits grouped by polarity
  (`polarity:dont` first — the rules you must not violate), `guide` hits as
  context. Flag any `stale` (cited code moved) or `drifted` (anchored file
  changed) and say "re-read before relying."

Then state the bottom line: how wide the blast radius is and which directives
constrain the edit.

`kenn status` reports the live snapshot; if there is no snapshot yet, run
`kenn index` once to build it, then retry.
