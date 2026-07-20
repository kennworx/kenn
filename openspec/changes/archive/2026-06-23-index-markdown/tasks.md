## 1. Data model & identity (`kenn-model`)

- [x] 1.1 Add a markdown discriminator to the language/prefix layer: `md` prefix in `Language` (or a parallel node-source) with `from_prefix`/`db_name`/`extensions(["md","markdown"])` wired. → verify: prefix round-trip test passes for `md`.
- [x] 1.2 Add `LinksTo` and `Embeds` to the `EdgeKind` enum and its payload-carrying variant (allow `LinksTo` to carry a `match_kind` grade and an optional `relation`). → verify: `as_str`/round-trip + edge-kind list include both kinds.
- [x] 1.3 Add `Kind::Document` (md file-as-node) and `Kind::Section` (heading) to the kind enum; both are symbol-space nodes carrying their `md:` native ID as `pub_id`. → verify: `db_name`/`from_db_name` round-trip for both kinds.
- [x] 1.4 Define the `md:<root-label>/<relpath>#<heading-slug>` ID builder + GitHub slugify with `-1/-2` in-file dedup. → verify: slug dedup + two-roots-same-relpath produce distinct IDs (unit tests).
- [x] 1.5 Persist the graded `LinksTo`/`Embeds` payload: `link_grade` + `link_relation` nullable columns on `edges`, stable `link_grade_code`, writer flatten arms. NOTE: edge payloads are **write-only** in the current store (no reader reconstructs `field_op`/`corr_*` either) — the read path for `grade`/`relation` lands with `check_links` (task 7.1). Section identity needs no special handling: a `document`/`section` symbol's `md:` native ID is its `pub_id`, a free string already accepted by the symbol identity path. → verify: writer test asserts both columns persist for `LinksTo` (grade+relation) and `Embeds` (grade). ✓

## 2. Roots, discovery & frontmatter collect (`kenn-config`, `kenn-indexer`)
<!-- NOTE: discovery/collect lives in kenn-indexer (the markdown producer),
     NOT kenn-collect — kenn-collect is the agent file-write tracker, unrelated. -->


- [x] 2.1 Add `MarkdownConfig` as its own field on `LanguageConfig` (`kenn-config`): `roots: Vec<Root { glob, label? }>` (search globs over files/dirs) + `excludes: Vec<String>` (raw exclude glob patterns); a root may be flagged/derived in-repo vs external by label. → verify: config parse test for in-repo + labeled external roots and excludes.
- [x] 2.2 Markdown walker discovers `.md` across configured roots: compile search/exclude globs to `GlobSet`s; a directory glob expands to `<dir>/**/*.md` (recursive); apply excludes. → verify: discovery test over a fixture tree (nested subdirs via dir glob, one in-repo + one external root, an excluded path skipped).
- [x] 2.3 Phase-1 collect: parse YAML frontmatter (`title`, `aliases`, `tags`, `related`) and scan heading lines for slugs, without full body parse. → verify: collect over a fixture returns frontmatter + heading slugs.
- [x] 2.4 Build the global resolution index `path / filename-stem / alias / title → md node id`. → verify: index lookups resolve a file by relpath, stem, alias, and title.

## 3. Markdown body parse → records (`kenn-indexer`)

- [x] 3.1 Add the markdown parser dependency (`comrak`/`pulldown-cmark`) + a frontmatter parser; pin versions. → verify: builds; parser smoke test on a sample doc.
- [x] 3.2 Walk a body into the heading tree: section nodes with prose spans (heading line → next same-or-higher heading), emitting `contains`/`defined_in` edges from heading levels (no enclosing-provider chain). → verify: nesting test — `# A > ## B > ### C` yields the containment tree.
- [x] 3.3 Emit file node (carrying frontmatter metadata) + section nodes + section prose into the `*DocsRecord` path so FTS + embeddings cover sections. → verify: prose flows to `SymbolDocsRecord` and reaches the store (ingest test); FTS-search/embedding query verification folds into 7.3.
- [x] 3.4 Emit records through `BatchSink` as a sibling producer (no SCIP/`transform`). → verify: a markdown-only run publishes nodes with no SCIP documents produced.
- [x] 3.5 Nested `Kind::Module` tree (design D13): a module per root + per directory, chained `child --defined_in--> parent`; each document `defined_in` its directory module, which `contains` the file (moved off the document). → verify: `module_ids_and_chain` (kenn-model) + `nested_directory_modules_chain_and_own_documents` (the root→docs→docs/a chain owns its document, browsable via `list_in_scope`/`list_inbound defined_in`).

## 4. Link resolution — md↔md (`kenn-indexer`)

- [x] 4.1 Extract link forms from bodies: inline `[t](target)`, reference links, wikilinks `[[t#a|alias]]`, same-file `[[#a]]`, transclusions `![[t]]`; classify external URLs out of the graph. → verify: extraction unit tests per form.
- [x] 4.2 Implement the resolution ladder (exact → drift → fuzzy → ambiguous → dangling), name-anchored on filename/stem, recording a `match_kind` grade on each edge; never drop a link. → verify: scenario tests for exact, drifted path, broken→external-stub.
- [x] 4.3 Emit `links_to` vs `embeds` edges (transclusion distinct). → verify: a host with both a `[[note]]` and `![[note]]` yields one `links_to` and one `embeds`.
- [x] 4.4 Backlinks: edges are emitted with section-granular src + resolved target; `list_callers`/`list_usages` (inbound-edge nav) serve them. Reader/nav verification folds into 7.3.

## 5. Cross-corpus md→code (`kenn-indexer`)

- [x] 5.1 Resolve file-target code links by basename (path-tolerant) and section/line targets to the enclosing code symbol. → verify: drifted code-file path resolves by basename + reports drift.
- [x] 5.2 Resolve symbol-target links by short name (reusing `find_symbol` tiers), with locality (nearest by path distance) tiebreak, else keep-all (edge per candidate). → verify: locality test picks nearer symbol; irreducible ambiguity emits N edges + reports ambiguous.
- [x] 5.3 Gate code-link resolution to in-repo roots only (guard the `resolve_code_link` call by root label) — enforced at the Group-6 wiring. → verify: external-vault code-looking ref emits no md→code edge. (Enforced in `discover` via `DiscoveredMarkdown.in_repo`: only in-repo files defer unresolved links; external-vault links dangle in phase 1 and never reach `resolve_code_link`. Tested by `external_vault_dangling_link_mints_stub`.)

## 6. Orchestration (`kenn-indexer`)

- [x] 6.1 Run markdown ingest as a parallel unit in the ingest phase, streaming through the bounded channel. → verify: run with code + markdown shows concurrent progress; md↔md resolved without code barrier.
- [x] 6.2 Add the post-code join barrier: md→code resolution begins only after all code ingest units complete, before finalize/publish; a code-less run skips the barrier. → verify: code→md backlink present after a mixed run; markdown-only run publishes with no barrier wait. (Markdown ingest split into `ingest_markdown_phase1` (in scope, md↔md + node emission, defers in-repo dangling) and `resolve_markdown_code` (post-join, store-backed `StoreCodeLookup` via `reader_from_writer`). Targets symbols + files, backlink-first per the ShortId-collision tradeoff. Tested by `md_to_code_link_resolves_and_backlinks` (mixed) and `markdown_unit_runs_in_pipeline` / `ingests_markdown_records_into_the_store` (code-less).)
- [x] 6.3 Measure full-rebuild wall-clock (parse + resolve, embeddings warm) at ~5k and ~10k markdown files before finalizing full-rebuild-on-save; record the numbers in the change. → verify: measurement captured; if over budget, open a follow-up for per-root rebuild / coarser debounce (design Open Question). **MEASURED:** a synthetic 10k-file corpus (nested dirs, frontmatter, ~6.5 wikilinks/file incl. drifted + dangling) full-rebuilds in **~2.0s** wall-clock (`kenn index`: discover → parse → md↔md resolve → modules → aggregate → finalize; embeddings deferred to the incremental `kenn embed` pass), ~90 MB peak RSS, producing 42.5k symbols / 115.8k edges. Well within a full-rebuild-on-save budget, so per-root incremental rebuild is **not** needed now; the watcher's full reindex (7.2) is fine. Embeddings are incremental (only null rows), so they don't add to the per-save cost. (5k is proportionally ~half; the 10k point is the one that gates the decision.)

## 7. MCP surface (`kenn-mcp`)

- [x] 7.1 Add the `check_links` tool (and/or build-report section): list drifted / fuzzy / ambiguous / broken links with written + resolved targets. Owns the **read path** for the `link_grade`/`link_relation` edge columns persisted in 1.5 (the first edge-payload reader). → verify: tool lists a seeded drifted link and a broken link correctly. (Added a `links_to_file` edge kind (design D8/D10) so md→code file targets hydrate from the files table — sound forward + backlink, no collision. `scan_link_diagnostics` (DbReader) reads the `link_grade` column; the `check_links` tool lists non-exact links, decoding dangling targets. Tested by `check_links_lists_non_exact_links` + `md_to_code_file_link_uses_links_to_file_edge`.)
- [x] 7.2 Extend the file watcher to treat `.md` under all configured roots (including external paths outside the workspace) as index-affecting. → verify: editing a vault `.md` triggers a reindex in a watcher test. (When `markdown.enabled`, the watcher's extension list includes `.md`/`.markdown` (in-repo) and registers extra recursive notify watches for each external-vault root; `md_event_passes` accepts vault `.md` events, respecting markdown excludes. Tested by `in_repo_md_passes_when_markdown_enabled` + `external_vault_md_event_passes_and_respects_excludes`.)
- [x] 7.3 Confirm existing nav/search tools return `md:` nodes unchanged (`search_symbols`, `semantic_search`, `list_in_scope`, `find_at_location`). → verify: navigation integration test over a markdown fixture. (`crates/kenn-mcp/tests/markdown_nav.rs`. Surfaced + fixed a real gap: the MCP `parse_kind`/`parse_language` hand-rolled the model mapping and omitted `document`/`section`/`markdown`, so md nodes came back as `variable`/`rust`; both now delegate to `Kind::from_db_name`/`Language::from_db_name`.)

## 8. Quality gates

- [x] 8.1 Integration test fixture: a small in-repo `docs/` + an external vault + code, asserting the end-to-end graph (nesting, md↔md, code↔md, drift report). → verify: integration test green. (`end_to_end_corpus_graph` in markdown/ingest.rs: in-repo nested `docs/a/` + an external `notes` vault + a code symbol/file; asserts module nesting, md↔md backlink, md→code symbol+file backlinks, the D6 in-repo-only gate (vault `[[OrderHandler]]` does NOT resolve to code), and the drifted/dangling link report.)
- [x] 8.2 `cargo clippy --workspace --all-targets` clean (zero warnings, pedantic included).
- [x] 8.3 `just crap-ci` green on touched functions (split resolver/walker into small helpers as needed).
- [x] 8.4 `cargo fmt --all` as the final step.
