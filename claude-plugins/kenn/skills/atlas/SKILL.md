---
name: atlas
description: Orient yourself in a codebase fast by reading kenn's generated structural map — packages, cross-package domains, cross-package contracts (interfaces and who implements them), and non-code document dirs — instead of blind-grepping. Use when you land in an unfamiliar or freshly cloned repo, need to "get up to speed", "understand this repo", find "where does X live" or "who implements this interface", and to re-anchor after a compaction.
---

# atlas — a structural map of the repo

Every `kenn index` writes an **atlas**: a short `index.md` plus one concept file
per code **package**, non-code **document** directory, cross-package **domain**,
and cross-package **contract** (an interface/base type and everywhere it is
implemented). It's the fastest way to grasp a codebase's shape before touching
it — read the map, then drill into what's relevant. The files are plain markdown,
so they're readable directly (and by non-Claude tools) even without kenn.

Every axis is also a QUERY — `kenn packages`, `kenn domains`, `kenn contracts`,
`kenn documents` — answering from the same snapshot the markdown was built from,
through the same selection rules, so the two agree by construction. Read the
bundle to orient; use the verbs when you want one entity's detail, a resolvable
id to act on, or an axis larger than a page.

## Steps

1. **Find the map.** Run `kenn index`; it (re)builds the bundle and announces it
   with a marked line — `atlas: <path>`. Grep that line: `<path>` is the map's
   `index.md`. Derive it from the output every time — never hardcode a location —
   so this works unchanged under `kenn index -d <dir>`, worktrees, and custom
   stores.

2. **Read `index.md` to orient.** Its header states the shape: package / domain /
   symbol counts, test ratio, languages, and the indexed commit. The counts are
   what the REPO has, so on a big repo they can exceed the documents present — a
   heading like `## Domains — 78, heaviest 24` means the page shows the 24
   heaviest and `kenn domains` reaches all 78. A heading with no such suffix is
   showing you everything.

   `kenn overview` reports two community counters and they are NOT the same
   question: `domains` is this axis — what you see here and what `kenn domains`
   lists — while `cross_anchor_communities` is a raw clustering diagnostic that
   counts every cluster touching two packages, including ones joined only by a
   shared vendored type. Expect them to differ severalfold in either direction;
   read `domains` when you mean architecture.

   Entries below are grouped by **role in the dependency graph**, foundation
   first — so the packages everything rests on are the ones you read first:
   - **`## Providers`** — depended on, depending on little. The foundation.
   - **`## Layers`** — both depended on and depending. The middle of the stack.
   - **`## Consumers`** — depending on much, little depends on them. Apps, entry
     points, leaves.
   - **`## Tests`**, then **`## Isolated`** (no cross-package coupling at all —
     vendored, dead, or not wired up).
   - **`## Documents`** — first-party non-code dirs (docs, specs, config).
   - **`## Domains`** — clusters that span packages (a feature or concept cutting
     across the tree), each with its package span and size.
   - **`## Contracts`** — first-party interfaces / base types implemented in more
     than one package (the system's cross-package extension points), each with
     how many implementers across how many packages. Widest span leads. Empty
     when a repo keeps its abstractions package-local (idiomatic for some
     languages) — its absence is itself a signal.

   Every package entry carries `(N used by · M deps)` — the counts its grouping
   was derived from, so you can judge the classification rather than trust it.
   Within a section the most-depended-on package leads.

3. **Drill into the concepts relevant to your task** via the markdown links. A
   package concept carries:
   - **Central symbols** — the package's most-connected types/functions, as an
     `ID | Location` table. Each `ID` is a resolvable symbol id.
   - **Used by** / **Depends on** — both coupling directions, as
     `Package | Weight | Relations` tables. The relation split is the useful
     part: `implements` marks a contract/implementer pair, while `calls` or
     `type_use` alone is ordinary consumption. Read **Used by** before changing
     anything — it is the blast radius. A heading like
     `## Used by — 100 packages, heaviest 24` means the list is capped and tells
     you by how much.
   - **Members** — its top files.
   A domain concept lists its central symbols and the packages it spans.
   A contract concept names the interface (its own `ID | Location`, so you can
   pull the contract itself), the package it is defined in, and — grouped by
   package — every implementer as an `ID | Location` row. Read it before changing
   a shared interface: it is the complete implementer set, so it is that
   interface's blast radius. "Where is X implemented across the tree" is a
   contract question; the package-coupling `implements` split only says two
   packages are related, not who.

4. **Pull real code for a symbol** with `kenn get <ID>` — the id straight from any
   table (a central symbol, a contract, or one of its implementers) — or reach for
   the `kenn` skill for callers, usages, and implementers.

5. **Build understanding in context, then re-anchor.** The map is a skeleton:
   flesh out what a concept means as you read its code and keep that in your
   working context (it isn't written back to the bundle). Reread `index.md` to
   re-orient when you move to a new area or after a compaction.

If `kenn` isn't on `PATH`, install it; if there's no index yet, `kenn index`
builds the atlas as a side effect of the first run.
