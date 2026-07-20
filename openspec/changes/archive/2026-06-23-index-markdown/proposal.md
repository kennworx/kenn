## Why

Markdown is invisible to kenn today — the index is SCIP-centric (5 code
languages), so standalone `.md` files are never walked, searched, or linked.
A repo's `docs/` and, more acutely, a PAI/Obsidian-style knowledge vault
(thousands of notes) used alongside a codebase are unmanageable without
full-text/semantic search and a navigable link graph. kenn already owns the
exact machinery a vault needs — FTS5, embeddings, a typed node/edge graph, and
the navigation tools over it — so the marginal cost is an ingestion path plus
link resolution; everything downstream is reused.

## What Changes

- Index markdown as a first-class corpus in the **same unified graph** as code:
  `md:` nodes sit beside `rs:`/`ts:`/… nodes in one store.
- **Section granularity with nesting**: each heading becomes a node; the
  `#`>`##`>`###` hierarchy is modeled with `contains`/`defined_in` edges
  (no enclosing-heuristic machinery needed — heading levels give the tree
  directly). Section prose feeds FTS5 + embeddings.
- **Frontmatter** (YAML) is parsed and stored as node metadata; `title` and
  `aliases` additionally drive link resolution.
- **Link graph as the primary deliverable**: inline links `[t](path#anchor)`,
  wikilinks `[[slug#anchor|alias]]`, and transclusions `![[…]]` become
  `links_to` / `embeds` edges. Backlinks fall out of the existing
  `list_callers`/`list_usages` tools.
- **Cross-corpus md→code links** (file and symbol targets), enabling the
  high-value **code→md backlink** ("what docs describe this symbol/file?").
- **Recall-first resolution ladder** that treats prose link-rot as drift, not
  failure: name-anchored matching (filename / symbol short-name) tolerant of a
  stale relative path or namespace; fuzzy fallback; keep-all on irreducible
  ambiguity; external stub only when no name matches. Every downgrade is
  recorded with a match-quality grade.
- **Link-health reporting** (`check_links`, cousin of `check_anchors`):
  surfaces drifted / fuzzy / ambiguous / broken links with both the written
  and resolved targets.
- **Two corpus modes**: markdown roots inside the repo, and external vault
  roots used alongside it. md→code resolution is gated to **in-repo roots**;
  external vaults get the md↔md graph only.
- Build is **two-phase** (collect frontmatter+headings → resolve+emit) and runs
  **in parallel** with code ingest; md→code resolution waits on a post-code
  **join barrier**. No incremental machinery — the snapshot rebuild re-resolves
  the whole corpus each run.

## Capabilities

### New Capabilities

- `markdown-index`: markdown as an indexed corpus — root discovery and the
  two corpus modes, the two-phase collect/resolve build running parallel to
  code ingest, frontmatter parsing into node metadata, section nodes and the
  heading-tree nesting, and section prose flowing into FTS5 + embeddings.
- `markdown-link-graph`: the link and embed edges and their resolution — the
  link taxonomy (inline / wikilink / transclusion / frontmatter-typed), the
  recall-first resolution ladder with match-quality grading, md→code resolution
  gated to in-repo roots behind the post-code barrier, and the `check_links`
  link-health report.

### Modified Capabilities

- `source-data-model`: extend the public-ID scheme with an `md:` prefix and a
  path/anchor-based native-ID form for markdown files (`md:<root>/<relpath>`)
  and sections (`…#<heading-slug>`); add `links_to` and `embeds` to the
  edge-kind enum; add markdown file/section node kinds.
- `code-intel-data-model`: add `links_to` and `embeds` to the enumerated
  edge kinds, and a markdown-section identity rule (a section's `md:` native ID
  serves as the `symbol_string` analog in the `(canonical_path, symbol_string,
  range)` key).
- `indexing-orchestrator`: markdown ingest runs as an additional parallel
  ingest unit during the ingest phase; md→code link resolution runs as a step
  gated on completion of all code ingest units, before finalize/publish.

## Impact

- **Crates**: `kenn-collect` (markdown file discovery + frontmatter scan),
  `kenn-indexer` (markdown walk → `kenn_model` records, link resolver, parallel
  ingest unit, post-code resolution step), `kenn-model` (`md:` id scheme,
  `LinksTo`/`Embeds` edge kinds, markdown node kinds), `kenn-config` (markdown
  root configuration + external vault roots), `kenn-mcp` (`check_links` tool;
  watcher extensions for `.md`), `kenn-store` (reuses node/edge/FTS/vec tables;
  must round-trip the new `LinksTo`/`Embeds` edge-property variants and accept a
  section node whose `symbol_string` is its `md:` native ID).
- **Dependencies**: a markdown parser (e.g. `comrak`/`pulldown-cmark`) and a
  YAML frontmatter parser.
- **Read surface**: existing navigation/search tools (`search_symbols`,
  `semantic_search`, `list_callers`, `list_usages`, `find_similar`,
  `list_in_scope`, `find_at_location`) light up for `md:` nodes with no
  per-tool change.
- **Model change, additive at the table level**: new node/edge kinds and a new
  ID prefix require a `kenn-model` + serialization change (notably a graded
  `LinksTo` edge-property variant per design D5) and store round-trip
  verification, but no table migration is anticipated.
- **Out of scope — embeddings**: section text flows into the existing doc/embed
  records; vector production is a separate subsystem (`incremental-embedding`,
  content-addressed) and is not modified here.
