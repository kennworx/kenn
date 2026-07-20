## 1. Bundle location & layout

- [x] 1.1 Resolve the design open questions and record the decision in `design.md`:
      the bundle path under `Layout` (must stay valid after `publish`) and
      gitignored-by-default vs committed. Verify: `design.md` Open Questions are
      replaced with a stated decision.
- [x] 1.2 Add an `atlas_dir()` resolver to the store `Layout` (kenn-store,
      `store-layout`), workspace-relative and post-publish stable. Verify: unit
      test resolves it under the store root for both a normal workspace and a
      `-d ./foreign` workspace (asserts the foreign path, not the invoking repo).

## 2. OKF writer primitives (kenn-indexer)

- [x] 2.1 Frontmatter serializer: non-empty `type` (required) + recommended keys
      (`title`/`description`/`resource`/`tags`) + arbitrary `kenn.*` producer keys.
      **No per-concept wall-clock `timestamp` in v1** (determinism — see 3.3). Verify:
      round-trips; `kenn.*` keys preserved; no `timestamp` key emitted. **Mutation-check
      (§9)**: force an empty `type`, confirm the conformance test (2.5) goes red.
- [x] 2.2 Concept-document writer: frontmatter block + markdown body → one file at
      a **path-qualified** concept-id via a **single shared `concept_id(anchor)`
      function** (also used by 3.2's dep links, so ids and links can't diverge),
      qualified by the unit's anchor path so two units sharing a leaf name don't
      collide. Verify: output parses as an OKF concept file; two units named `foo`
      under different anchors get distinct ids; a dep link resolves to a real id.
- [x] 2.3 `index.md` writer (NO frontmatter — OKF reserved): a grouped, linked
      concept list under a markdown shape/status header (languages, package count,
      symbol count, test ratio, a concrete freshness signal = HEAD sha or
      `StalenessKey` + ISO-8601 build timestamp, total concept count). Verify: the
      header contains each named field and the file has no YAML frontmatter.
- [x] 2.4 `log.md` writer (NO frontmatter): **append-preserve** — prepend a new
      dated section per index run (newest-first, ISO-8601 headings) while retaining
      prior sections; concept docs + `index.md` regenerate, `log.md` does not wipe.
      Verify: two successive indexes yield two dated sections, newest first, with
      the first run's entry still present.
- [x] 2.5 Conformance check (test util): every non-reserved `.md` has non-empty
      `type`; reserved files follow OKF structure. Verify: passes over a produced
      bundle; used by 2.1's mutation-check.

## 3. Package concept producer (kenn-indexer)

- [x] 3.1 Build one concept per **internal (non-external) package** — exclude
      `packages` rows that are external deps; manifest-less anchors are deferred.
      Each carries: anchor + `resource` (manifest, else the anchor dir), symbol
      count, central symbols = the package's **own non-test** symbols ranked by
      weighted degree **recomputed from the raw `edges`** via the Reader (summed
      incident edge weight; NOT the global `analysis_god_nodes`), top
      member files (`files`/`defs`), and `description` seeded **verbatim** from the
      root module doc (`file_docs`) via the language-keyed root-file rule (3.4),
      empty when absent. Verify: unit test over a fixture graph — an external
      package gets no concept; central symbols are this package's top-degree members
      (a globally-central symbol in *another* package does not appear); `description`
      equals the module doc verbatim, empty when none. **Mutation-check (§9)**: break
      the verbatim copy, confirm the test fails.
- [x] 3.2 Emit **directed** dependencies as bundle-relative markdown links: roll up
      the raw directed `edges` (primarily `EdgeKind::Imports`) by anchor keeping
      src→tgt — NOT `aggregate_edges`, which is undirected (`min_id`/`max_id`).
      Reuse the existing `resolve_anchors` mapping, and emit each link via the shared
      `concept_id` function (2.2) so links match ids. Verify: an A→B import yields a
      link to B in A's body and NOT a link to A in B's body, and the link path equals
      B's concept id. **Mutation-check (§9)**: collapse to undirected, confirm the
      direction assertion fails.
- [x] 3.3 Bodies carry structural content only (central symbols, dep links, members)
      — no interpretive prose, and **no wall-clock** anywhere in a concept doc, so the
      doc is deterministic (re-index of an unchanged repo = no-op diff). Verify: the
      producer exposes no summary/prose field beyond the seeded (verbatim)
      `description`; indexing twice with no source change produces byte-identical
      concept files.
- [x] 3.4 Language-keyed root-file selection (`pick_root_file` in atlas/producer.rs):
      pick each package's root module (Rust `lib.rs`/`main.rs`, TS `index.ts`,
      Python `__init__.py`, Go `doc.go`), highest-precedence-first, ties broken by
      shallowest then lexicographic path (determinism); `None` when the language has
      no convention or no file matches. ✓ `pick_root_file_is_language_keyed_and_deterministic`.

## 4. Wire into the shared producer (both orchestration paths)

- [x] 4.1 A `finalize_atlas(layout, run_id, config)` helper in kenn-indexer, called
      from the one point both `cmd_index::run_async` (CLI) and
      `workflow::index_workspace` (MCP) reach after the run's code graph is persisted
      and before/at `publish`. It reads the run's `code.db` via the Reader API (no
      dependency on the analysis pass), builds the bundle into the run dir, and it is
      carried on `publish`. Verify: CLI e2e AND an MCP-path test both write the
      bundle; a test with the analysis pass disabled still writes it.
- [x] 4.2 The producer reads raw tables via the Reader (packages/symbols/files/
      edges/docs) and derives directed deps (3.2) + per-symbol centrality (3.1) by
      recomputing weighted degree from raw `edges` — it does NOT read the undirected
      `aggregate_edges` or the global `analysis_god_nodes`. Verify: an index with
      analysis disabled still yields correct central symbols + directed deps.
- [x] 4.3 The producer degrades gracefully if an analysis input is missing: confirm
      the aggregate stage runs on every index; if central-symbol data is absent, a
      concept still emits (empty central-symbols) rather than failing the index.
      Verify: a test with the analysis input stubbed empty still writes a conformant
      bundle.

## 5. The markdown handle

- [x] 5.1 `kenn index` announces the **published** atlas `index.md` path (after the
      snapshot flip) on the existing output channel: a **marked** line with a stable
      greppable prefix (`atlas: <path>`) in human mode, a field on the completion
      event under the existing `--json` mode — never a bare line into the JSON
      stream. Verify: `cli_smoke` (human) greps the marker and resolves it to the
      published `index.md`; a `--json` run has the path as a field and no stray
      markdown line. **Mutation-check (§9)**: remove the announce, confirm the smoke
      test fails.

## 6. Consumption skill

- [x] 6.1 Author `claude-plugins/kenn/skills/atlas/SKILL.md` (agentskills.io format,
      matching the eight existing kenn skills): trigger-rich `description`
      (orient / understand this repo / get up to speed / freshly cloned) and
      path-free `## Steps` that derive the atlas location from `kenn index` output
      and enrich **in-context** (v1: understanding is built to orient, not written
      back to the bundle — see design D8). Verify: the file has no literal bundle
      path (grep), and its `description` matches the orientation intents.

## 7. Verify end-to-end + gates

- [x] 7.1 E2E: index this repo, then assert a conformant bundle with one concept
      per internal package (no concept for external deps), the marked handle line,
      and a re-readable `index.md` whose links resolve to concept files. (A
      `just atlas-smoke` recipe or an integration test.)
- [x] 7.2 Rust gates before done: `cargo clippy --workspace --all-targets` clean,
      `just crap-ci` green, then `cargo fmt --all` as the last step.

## 8. Post-proposal evolution (what actually shipped)

The atlas shipped, but two decisions evolved past §§1–7 (kept above as the
original plan); this section records reality.

- [x] 8.1 **Central symbols from the directed weighted aggregate graph** —
      supersedes 3.1/4.2's "recompute weighted degree from raw `edges`, NOT
      `aggregate_edges`". Ranking over `aggregate_nodes`/`aggregate_edges` (already
      weighted, containers collapsed) ranks real types instead of the namespaces a
      raw incidence count crowned. Container kinds (namespace/module/package) are
      excluded; a production package hides its test classes, a **test-dominant**
      package includes them + a `tests` tag. (`atlas/producer.rs` unit tests.)
- [x] 8.2 **Document concepts** — first-party non-code directories (`openspec`, `docs`,
      …) emit `type: document` concepts so the map isn't code-only.
- [x] 8.3 **Domains axis** (the second axis) — `type: domain` documents for
      cross-package flat-Louvain communities, read back from the persisted
      `analysis_flat_communities` + `analysis_node_membership` on the writer's own
      connection (atlas ⊥ kenn-analyze — consumes tables, never recomputes). The
      analysis hook runs before the atlas so the tables exist; a community qualifies
      only if it spans >1 package after excluding container/test nodes; hub =
      highest-weighted-degree eligible member. (producer unit tests + an
      `aggregate_integration` e2e + a byte-identical determinism test.)
- [x] 8.4 Per-package `description` seeded from the root-module doc (tasks 3.1/3.4):
      a `scan_file_docs` reader (the `file_docs` table) is threaded through
      `build_concepts`; each package concept's `description` is its root file's
      module doc, verbatim, `None` when absent (never synthesized). ✓
      `package_description_seeds_from_root_module_doc_verbatim` + **§9 mutation-check**
      (non-verbatim copy fails). Determinism preserved (integration byte-identical
      re-run still green).
