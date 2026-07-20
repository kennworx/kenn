---
name: atlas
description: Orient yourself in a codebase fast by reading kenn's generated structural map — packages, cross-package domains, and non-code document dirs — instead of blind-grepping. Use when you land in an unfamiliar or freshly cloned repo, need to "get up to speed", "understand this repo", or find "where does X live", and to re-anchor after a compaction.
---

# atlas — a structural map of the repo

Every `kenn index` writes an **atlas**: a short `index.md` plus one concept file
per code **package**, non-code **document** directory, and cross-package
**domain**. It's the fastest way to grasp a codebase's shape before touching it —
read the map, then drill into what's relevant. The files are plain markdown, so
they're readable directly (and by non-Claude tools) even without kenn.

## Steps

1. **Find the map.** Run `kenn index`; it (re)builds the bundle and announces it
   with a marked line — `atlas: <path>`. Grep that line: `<path>` is the map's
   `index.md`. Derive it from the output every time — never hardcode a location —
   so this works unchanged under `kenn index -d <dir>`, worktrees, and custom
   stores.

2. **Read `index.md` to orient.** Its header states the shape: package / domain /
   symbol counts, test ratio, languages, and the indexed commit. Entries below
   are grouped into sections:
   - **by language** (`## Rust`, `## C#`, …) — the code **packages**, one link each.
   - **`## Documents`** — first-party non-code dirs (docs, specs, config).
   - **`## Domains`** — clusters that span packages (a feature or concept cutting
     across the tree), each with its package span and size.

3. **Drill into the concepts relevant to your task** via the markdown links. A
   package concept carries:
   - **Central symbols** — the package's most-connected types/functions, as an
     `ID | Location` table. Each `ID` is a resolvable symbol id.
   - **Depends on** — outgoing package dependencies (links to their concepts).
   - **Members** — its top files.
   A domain concept lists its central symbols and the packages it spans.

4. **Pull real code for a central symbol** with `kenn get <ID>` (the id straight
   from the table) — or reach for the `kenn` skill for callers, usages, and
   implementers.

5. **Build understanding in context, then re-anchor.** The map is a skeleton:
   flesh out what a concept means as you read its code and keep that in your
   working context (it isn't written back to the bundle). Reread `index.md` to
   re-orient when you move to a new area or after a compaction.

If `kenn` isn't on `PATH`, install it; if there's no index yet, `kenn index`
builds the atlas as a side effect of the first run.
