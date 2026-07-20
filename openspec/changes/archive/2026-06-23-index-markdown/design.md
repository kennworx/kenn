## Context

kenn's indexing pipeline is SCIP-centric: `kenn-collect` runs per-language SCIP
indexers, `kenn-indexer` transforms SCIP `Document`s into `kenn_model` records
(`FileRecord`, `SymbolRecord`, `DefRecord`, `EdgeRecord`, `*DocsRecord`), and a
`BatchSink` streams them into the store (SQLite + FTS5 + vec0). Everything
**downstream of `kenn_model` records** — the node/edge/FTS/vec schema, the
embeddings, and all MCP navigation/search tools — is generic graph machinery
that does not know its producer was SCIP. Markdown is currently invisible:
the only "docs" indexed are comments attached to code symbols.

Reindex is a **whole-corpus snapshot rebuild** of *structure* with an atomic
hot-swap (`arc_swap` of the reader binding); there is no per-file structural
delta. This is load-bearing for the design: link resolution is just a build pass
and never has to be maintained incrementally. **Embedding is a separate
subsystem** — `incremental-embedding` keys vectors by `xxh3-64(embeddable_text)`
in a content-addressed sidecar, so unchanged section text is reused without a
model call. This change feeds section prose into the existing doc/embed records;
it does not touch vector production.

## Goals / Non-Goals

**Goals:**

- Markdown becomes first-class nodes/edges in the *same* graph as code.
- Section (heading) granularity with the heading hierarchy modeled.
- A navigable link graph — md↔md and md→code — as the primary deliverable,
  with backlinks served by existing tools.
- Resolution that survives prose link-rot: stale paths/qualifiers resolve as
  *drift*, not breakage, and every downgrade is reported.
- Two corpus modes: in-repo markdown and external vaults used alongside a repo.

**Non-Goals:**

- Incremental / per-file markdown *structure* reindex (kenn has none; full
  rebuild stands).
- Embedding/vector production — owned by `incremental-embedding`; this change
  only emits the section text it consumes.
- Authoring markdown — this indexes *existing* markdown read-only; it does not
  merge with or replace the findings store.
- Content roll-up of transcluded sections into the host (modeled as an edge,
  deferred as a search behavior).
- Auto-fixing drifted links (the report makes it possible later).
- Cross-repo code resolution for external vault roots.

## Decisions

### D1 — Native record emission, not a SCIP shim

A markdown walker (`comrak`/`pulldown-cmark`) emits `kenn_model` records
directly (`FileRecord` for the `.md`, section nodes, `contains`/`defined_in`
edges for nesting, `links_to`/`embeds` edges, prose into `*DocsRecord` for
FTS+embeddings). It is a **sibling producer** to the SCIP path, sharing only
`records → sink → store`.

*Alternative — markdown→SCIP shim:* emit synthetic SCIP so the existing
`transform/` is reused. Rejected: SCIP is a code protocol; encoding headings as
symbols and links as occurrences fights the format, and `transform/` stays
cleanly code-only this way.

### D2 — Section granularity; heading tree is free

Each heading is a node; the `#`>`##`>`###` levels give the `contains` /
`defined_in` tree directly from the parse. The SCIP enclosing-provider chain
(`enclosing.rs`, Tier 1–3 positional heuristics) is **not** used — markdown
nesting is unambiguous. A section node owns its prose span (heading line → next
same-or-higher heading); that span is the FTS + embedding unit, matching the
"embed prose, not signatures" principle. The file node carries title +
frontmatter for whole-file hits.

### D3 — `id` scheme carries the corpus root

`md:<root-label>/<relpath>#<heading-slug>` where `<root-label>` is `workspace`
for in-repo markdown or a configured label per external vault. Heading slugs use
GitHub slugification with `-1`/`-2` dedup within a file. Slug-based (not
line-based) ids survive prose edits and break only on heading-text changes —
the same stability trade-off as a code-symbol rename. The root label keeps
multiple corpora addressable and keeps code↔md targets unambiguous.

### D4 — Two-phase build, parallel with code, post-code barrier

```
   ┌──────────── parallel ingest units ─────────────┐
   │ rust ts go py c# (SCIP)  │  markdown            │
   │                          │  P1 collect:         │
   │                          │   frontmatter,       │
   │                          │   heading slugs      │
   │                          │  P2 resolve+emit:    │
   │                          │   md↔md links NOW ───┼─► resolved
   └─────────────┬────────────┴──────────────────────┘
                 ▼  JOIN: all code units complete
        md→code resolution (in-repo roots only) ───────► resolved
                 ▼
            sink → snapshot swap
```

- **Phase 1 (collect)** scans every `.md` cheaply: parse YAML frontmatter
  (`title`, `aliases`, `tags`, typed `related`) and scan heading lines for
  slugs. Builds the **global resolution index**: `path / filename-stem / alias /
  title → md node id`. Aliases/title must be known before any link resolves —
  hence collect-first.
- **Phase 2 (resolve + emit)** full-parses bodies: heading tree (section
  spans), links, embeds. md↔md targets resolve immediately (the md corpus is
  self-contained once collected).
- **md→code** resolution runs only after all code ingest units finish (their
  symbols/files must exist), as a post-code step before finalize.

### D5 — Recall-first resolution ladder, name-anchored

Prose links rot predictably: the human-meaningful name (filename, symbol
short-name) survives; the qualifier (directory path, namespace/package) drifts.
Resolution is therefore name-anchored and qualifier-tolerant:

```
  quality     condition                                   edge?    reported?
  ─────────   ─────────────────────────────────────────   ──────   ─────────
  exact       path + name both current                     1        no
  drifted     name current, path/qualifier stale           1        yes
  fuzzy       name approximate (case/typo/partial)          1        yes (low-conf)
  ambiguous   N name matches, locality can't disambiguate   N        yes (kept all)
  dangling    no name match                                 external yes (broken)
```

Algorithm: try exact → strip the volatile part and match the stable part
(basename / short-name) → fuzzy → keep-all → dangle. For symbols with N matches,
**locality** (nearest by path distance to the linking `.md`) breaks ties; if
still ambiguous, **keep all** (emit an edge per candidate). The edge carries a
`match_kind` reusing `find_symbol`'s existing vocabulary
(exact/prefix/case-insensitive/fuzzy), so "confident links only" is a filter and
drift is queryable data. Rationale: over-linking surfaces a doc under a
near-namesake (cheap, visible); dropping a link loses a real reference silently.

### D6 — Code resolution gated to in-repo roots

In-repo markdown gets md↔md + md→code. External vault roots get md↔md only;
code-looking refs stay text/external. An external vault may be used across many
repos — resolving its refs against *this* checkout would be a guess.

### D7 — `embeds` distinct from `links_to`

`![[…]]` transclusion inlines the target's content (host's effective content ⊇
target); `[[…]]` merely references. Modeled as a separate `embeds` edge kind so
"what is reused/inlined where" stays distinct from "what references this," and
so a later decision to roll transcluded prose into the host's search unit needs
no migration.

### D8 — Link-health report (`check_links`)

A read tool/build-report section, cousin of `check_anchors`, listing
drifted / fuzzy / ambiguous / broken links with both the written target and the
resolved one. Because both are known, this is one step from an auto-fix
(rewrite the stale path/qualifier), analogous to `record_anchor`'s rename.

*Implemented:* `check_links` is the **first edge-payload reader** — it reads back
the `link_grade` column persisted in 1.5 via `scan_link_diagnostics` (a `DbReader`
inherent query). It scans `links_to`/`embeds`/`links_to_file` edges whose source
is a markdown node and whose grade is not `exact`, hydrating each target *by edge
kind*: `links_to_file` from the files table, the rest from symbols (a resolved
symbol, or a dangling stub whose `pub_id` carries the written target, decoded for
display). A dangling row shows the written target; a drifted/ambiguous row shows
the resolved one.

### D9 — Markdown joins the `Language` enum

Markdown is represented as a new `Language::Markdown` variant (prefix `md`)
rather than a parallel node-source type. This keeps one identity/prefix path and
lets every existing consumer treat `md:` nodes uniformly. The cost is exhaustive
`match` arms across the enum's methods (`extensions` → `["md","markdown"]`;
`project_files` → empty; no SCIP driver), which are filled explicitly.

*Alternative — a separate node-source enum:* avoids the empty/awkward arms but
forks identity and forces every `Language` consumer to handle two node origins.
Rejected: the per-arm cost is bounded and local; a forked identity path is not.

`Language::partition()` assigns Markdown a stable partition index `5` (existing
`0`–`4` unchanged) so `md:` short-ids never collide with code ids.

### D10 — Markdown file and sections are `SymbolRecord` nodes

A `ShortId` is `[partition | counter]` with **no file/symbol discriminator**;
within a partition the file and symbol counters overlap, and the existing graph
disambiguates an edge target only *by edge kind* (only `contains` targets the
files table). A `links_to` edge can target a file, a section, or code, so that
trick does not extend to markdown.

Therefore the markdown **file node and each section are `SymbolRecord`s** in the
symbol id space, with new kinds `Document` (the file-as-node, `pub_id`
`md:<root>/<relpath>`) and `Section` (a heading, `pub_id` `…#<slug>`). Nesting
uses `enclosing_sym_id` / `defined_in` / `contains` among the document and its
sections. A `FileRecord` (language Markdown) is still emitted for the files
table + `content_hash` change detection, but **link edges never target it** —
they target the `Document`/`Section` symbols, so every `links_to`/`embeds` edge
resolves unambiguously and the whole symbol-nav surface (`list_callers`,
`list_usages`, `get_symbol`, `find_at_location`, `enclosing_sym_id` nesting)
works for markdown with no per-tool change.

*md→code file links* (`[x](../src/order.rs)`) target a code **file** node, which
collides with a symbol id. Resolved (Group 7) by giving them a distinct
`links_to_file` **edge kind** (sibling of `contains`) — the graph's existing
file-vs-symbol disambiguator is the edge kind, so file targets hydrate from the
files table and both forward nav and the code-file→md backlink are sound, with no
exclusion. `CodeTarget` carries `is_file` to pick the kind at emit time. (This
supersedes the earlier "resolve to a top-level module symbol" sketch.)

### D11 — Markdown discovery uses search + exclude globs (dir = recursive)

Markdown roots are configured with the same glob model as code search paths:
**search globs** (include) plus **exclude globs**, over files or directories. A
glob naming a **directory** means "index every `.md` beneath it, recursively"
(`<dir>/**/*.md`); a glob may also name individual files. Excludes are globs that
remove matches from the discovered set.

Unlike code — whose excludes run at SCIP-transform time via
`Workspace::is_excluded` — markdown is a native producer (D1) that **owns its own
discovery**, so the markdown walker applies include + exclude globs **directly**.
The config model is shared with code; the code path is markdown's own.
Consequently `Workspace`'s per-language exclude fields stay code-only and
`Workspace::is_excluded(Markdown, …)` is never consulted (returns `false`).
External vault roots additionally carry a label for identity (D3).

### D12 — Markdown config is its own `MarkdownConfig` field

Per-language config in `kenn-config` is heterogeneous named fields
(`CsharpConfig`, `RustConfig`, … each a distinct type with distinct fields:
`command`, `targets`, `provision_directory_build_props`), *not* a
`HashMap<Language, Config>` — a map would force a single value type that does
not exist. Markdown's config is a third shape again (roots with labels, exclude
patterns, **no `command`/no external indexer/no SCIP**), so it joins as its own
field rather than a uniform map value:

```rust
pub struct LanguageConfig {
    pub csharp: CsharpConfig, pub rust: RustConfig,
    pub typescript: TypescriptConfig, pub python: PythonConfig,
    pub markdown: MarkdownConfig,   // roots: Vec<Root { glob, label? }>, excludes: Vec<String>
}
```

`MarkdownConfig` holds **raw** glob pattern strings (serde form), mirroring how
the code configs hold `excludes: Vec<String>`. The markdown **walker** compiles
them into `GlobSet`s at discovery time — the same raw→compiled projection the
code path does at the CLI seam (`cmd_index.rs` → `Workspace::with_language_excludes`),
except markdown compiles in its own walker because it owns discovery (D11), not
via `Workspace`. `Workspace`'s compiled per-language excludes stay code-only and
could independently become a `HashMap<Language, GlobSet>` (homogeneous values) —
an optional cosmetic cleanup, out of scope here.

### D13 — Markdown roots and directories are nested `Kind::Module` nodes

A markdown corpus is otherwise a flat heap of `Document` nodes — unbrowsable at
the 10k-file scale that motivated this change. The code graph already solves
"browse a named container of things" with **modules**: a `Kind::Module`
`SymbolRecord`, parents linked by `enclosing_sym_id` + `defined_in`, files owned
by `contains`, and `list_in_scope` walking it a level at a time (code namespaces
nest arbitrarily — `mod a::b::c`, `namespace Foo.Bar`). Markdown reuses it
verbatim rather than growing a parallel grouping:

- One `Kind::Module` per **root** (`md:<label>`) and per **nested directory**
  (`md:<label>/<dir>`), deduped across files and chained
  `child --defined_in--> parent` up to the root (`module_chain` /
  `module_id` in `kenn-model/id/md.rs`).
- Each `Document`'s `enclosing_sym_id` is its directory module, with a
  `document --defined_in--> module` edge; the module `contains` the file (the
  file-owning edge moves from the document to the module — `contains` stays the
  only file-targeting kind, D10).
- Modules carry **no def** (a directory is not a source span) and, unlike code,
  are minted **explicitly** (one per dir, always a full node), so none of the
  SCIP stub machinery is needed.

Result: `list_in_scope("md:notes/daily")` browses a folder, `get_workspace_overview`
surfaces each corpus root, and per-module stats/analysis come for free — all
through the existing module surface, no new MCP/reader code. Flat-vs-nested was a
choice; nested wins because folder drill-down is what makes a large vault
navigable, and it costs only one node per directory plus the chain edges.

## Risks / Trade-offs

- **`md:` prefix + new edge kinds touch shared schema** (`source-data-model`,
  `code-intel-data-model` edge enum) → keep deltas additive; markdown node/edge
  kinds are new variants, not changes to code semantics. The graded `LinksTo`
  edge (D5) needs a new `EdgeProperties::LinksTo { match_kind, relation }`
  variant; the store must round-trip it. A section node has no code
  `symbol_string` — its `md:` native ID serves as the identity analog in
  `(canonical_path, symbol_string, range)`; verify the dedup path accepts it.
- **Wikilink ambiguity is inherent** → declared resolution order
  (exact path → filename-stem shortest-unique → alias → title), deterministic
  tiebreak, always logged; never a silent pick.
- **Slug ids break on heading-text edits** → accepted; same class as a symbol
  rename, and the full rebuild re-resolves anyway.
- **md→code symbol over-linking** (keep-all) inflates some backlink sets →
  bounded by locality-first; visible via `match_kind` filter and `check_links`.
- **Build ordering coupling** (md→code waits on code) → a single join barrier,
  not fine-grained; md↔md is unaffected and proceeds in parallel.
- **External vault watching** (a dir outside the repo) → watcher must accept
  configured roots beyond the workspace; full rebuild keeps it simple.

## Open Questions

- **Rebuild cost at vault scale** — a full-corpus parse + resolve fires on every
  `.md` save (watcher). Embeddings are cached, but parse/resolve is not.
  **Measure** wall-clock at ~5k and ~10k files before committing to
  full-rebuild-on-save; if too slow, revisit (per-root rebuild, coarser
  debounce, or a structural delta for markdown).
- md→code **symbol-link syntax**: path-only (deterministic) is the safe v1;
  bare-name/wikilink-to-symbol relies on D5's fuzzy tier — ship name-matching in
  v1 or gate behind a flag?
- Should typed **frontmatter `related:` links** (`supports`/`contradicts`/
  `extends`, the PAI knowledge-graph semantics) ship in v1 as
  `links_to{relation}`, or land after structural links prove out?
- Tags (`#tag`) and Obsidian block refs (`^blockid`) — index as metadata/edges
  now, or defer?
