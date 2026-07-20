## 1. Crate scaffold

- [x] 1.1 Create `crates/kenn-indexer/` with Cargo.toml, lib.rs, basic CI green
- [x] 1.2 Add workspace `Cargo.toml` at repo root pinning toolchain (`rust-toolchain.toml` with stable)
- [x] 1.3 Add dev dependency on `insta` (snapshot tests) and `assert_cmd` (later, for CLI wrapper)
- [x] 1.4 Add a placeholder `Sink` integration test that drains records into an in-memory vector

## 2. Data model types (~~`kenn-data-model` capability~~ — **SUPERSEDED**)

The standalone `kenn-data-model` capability defined here is **superseded by the
`source-data-model` proposal**. The producer crate now consumes `kenn-model`'s public
types (`SymbolRecord`, `EdgeRecord`, `EdgeKind`, `Kind`, `Language`, `IdTransformer`,
`PublicId`, ...) directly. Notable replacements:

- `SymbolKind` → `kenn_model::Kind` (22-variant closed enum, no `Unknown`)
- `EdgeKind` → `kenn_model::EdgeKind` (11 variants; `throws`/`catches`/`extends` are
  deferred per source-data-model "Deferred Capabilities")
- `EdgeRecord` is **pair-deduplicated** (no per-site `location`, no `count`) — see
  source-data-model D10
- `OccurrenceRecord` is **not persisted** — it's an ingest-time in-memory intermediate
  (source-data-model D14). The producer materializes occurrences during edge derivation,
  then discards them.
- `SymbolId` (verbatim SCIP wrapper) → `kenn_model::id::PublicId` + per-language
  `IdTransformer` impls (already implemented and tested)

Only producer-specific tasks remain in this section:

- [x] ~~2.1-2.7 Data-model record types~~ — superseded by `kenn-model` crate
- [x] 2.8 Define `Sink` trait — **batch-shaped**: `begin_run`, `write_batch(&RecordBatch)`, `end_run`. The producer streams SCIP documents one at a time but pushes records through a `BatchingSink<S>` adapter that flushes at a configurable threshold (default 10k records, see `[ingest] batch_size`). Failure semantics are simple: a failed batch ⇒ caller drops the DB and restarts (per user direction).
- [x] 2.9 Implement `VecSink` (collects every batch, for tests) and `BatchingSink<S>` adapter. ~~`JsonlSink`~~ removed: storage is SurrealDB-only, so the producer hands batches to a `SurrealdbSink` (lives in `indexed-store-and-lifecycle`) directly.
- [x] 2.10 Snapshot test: serialize each `kenn_model` record type emitted by the producer to JSON; lock the wire format

## 3. Path canonicalization

- [x] 3.1 Implement `Workspace { root: PathBuf, exclude_globs: Vec<Glob> }` with method `canonicalize(project_root_uri, relative_path) -> Result<WorkspaceRelativePath>`
- [x] 3.2 Reject paths outside workspace root with structured error
- [x] 3.3 Reject paths matching any explicit `exclude_globs` (default: `node_modules/`, `bin/`, `obj/`, `target/`)
- [x] 3.3a Implement `discover_other_worktrees(workspace_root) -> Vec<PathBuf>` via `git worktree list --porcelain` (using `gix` if surface is sufficient, else `git` subprocess); return empty when not in a git repo
- [x] 3.3b Combine other-worktree paths into the runtime exclude set; ensure the workspace root itself is never excluded even if it is a linked worktree
- [x] 3.4 Test: project_root under sub-dir, relative path canonicalization → workspace-relative
- [x] 3.5 Test: path outside workspace root → error
- [x] 3.6 Test: linked worktree at `/repo/wt/feature-x` (non-conventional path) excluded via git query
- [x] 3.6a Test: linked worktree at an arbitrary path is excluded via git query, not via any hard-coded path-name convention (`.worktrees/`, `wt/`, etc.). The exclusion is name-agnostic — git is the authority.
- [x] 3.6b Test: workspace root that IS a linked worktree is itself indexed; other linked worktrees under it are excluded
- [x] 3.6c Test: non-git workspace skips the git query and only honors explicit excludes
- [x] 3.7 Document case-folding behavior on macOS / Linux / Windows

## 4. SCIP protobuf streaming parser

- [x] 4.1 Add `scip` crate (protobuf types) and `prost` to Cargo dependencies
- [x] 4.2 Implement `parse_scip(reader) -> impl Iterator<Item = Document>` that streams documents one at a time, never holding the full Index in memory
- [x] 4.3 Implement `transform_document(doc, workspace) -> impl Iterator<Item = (SymbolRecord | OccurrenceRecord | EdgeRecord)>` for explicit relationships
- [x] 4.4 Test: parse the the small C# sample `index.scip` from the spike; assert document count, symbol count, edge count match `scip stats` output
- [x] 4.5 Test: peak memory while parsing the the captured 67 MB `index.scip` is bounded (use a memory probe or RSS sample)

## 5. Edge derivation

- [x] 5.1 Build a per-document index `defs: BTreeMap<Range, SymbolId>` of definition occurrences sorted by start position
- [x] 5.2 For each non-definition occurrence, find the smallest enclosing definition range via binary search
- [x] 5.3 Implement `classify_edge_kind(occurrence, enclosing_def, syntactic_hints) -> EdgeKind` with initial rules: token preceded by `new` → `instantiates`; in field-decl type position → `field_type`; in method-invocation position → `calls`; default → `references`
- [x] 5.4 Skip pseudo-symbols (SCIP `local N` patterns)
- [x] 5.5 Skip occurrences targeting symbols defined zero or multiple times in the workspace (would create file→file noise)
- [x] 5.6 Test: scenarios from `kenn-data-model/spec.md` (field type, method invocation, fallback to `references`)

## 5b. Enclosing-range provider (positional refinement, no AST parser)

- [x] 5b.1 Define `EnclosingProvider` trait with `attribute_from(canonical_path, occ_line, occ_col, defs_in_doc) -> Option<Symbol>` returning the FROM symbol for a non-def occurrence
- [x] 5b.2 Implement `ScipEnclosingProvider` that returns SCIP's `Occurrence.enclosing_range` when non-empty (tier 1)
- [x] 5b.3 Implement `CsharpPositionalRefinement` (tier 2) per the `scip-indexer/spec.md` requirement:
  - 5b.3.1 Skip parameter-kind defs (descriptor leaf `(name)`) when collecting per-doc def candidates
  - 5b.3.2 Read source file once per document; classify each line as `Blank | Comment | Preprocessor | Attribute | AttributeCont | Code` with bracket-balanced multi-line attribute support
  - 5b.3.3 Disambiguate attribute lists from C# 12 collection literals: a line beginning with `[` is `Code` (collection literal) when the previous non-blank, non-comment line ends with `=`, `=>`, `,`, or `(`; otherwise `Attribute`
  - 5b.3.4 Inline-attribute handling: a line whose `[...]` closes within the same line and is followed by non-trivial code on the same line classifies as `Code` (so step 5b.3.6 picks the same-line def)
  - 5b.3.5 For an occurrence on `Attribute` / `AttributeCont` line, advance its effective position to the next `Code` line before running the heuristic
  - 5b.3.6 Same-line forward-def: if there's a def on the (post-advance) line whose identifier column ≥ occurrence column, pick the smallest-column such def directly
  - 5b.3.7 Else fall back to last-preceding-def heuristic over the parameter-filtered candidates
- [x] 5b.4 (Future, when Rust driver lands) Implement `RustPositionalRefinement` adapting the C# rules for `#[attr]` syntax
- [x] 5b.5 Compose the providers into a chain per language driver: SCIP → language-specific refinement → bare last-preceding-def (only when source missing on disk)
- [x] 5b.6 Cache classified line-kinds per document (don't re-classify for each occurrence)
- [x] 5b.7 Test: scenarios from `scip-indexer/spec.md` (SCIP populates / C# attribute / collection literal / parameter exclusion / same-line forward def / source missing)
- [x] 5b.8 Regression fixture — a small C# file mixing `[DataMember]` properties, `[DisplayValue]` enum members, C# 12 collection-literal initializers, and inline `[Attr] decl`; assert FROM attribution matches expected symbols (codified from the the C# spike's residual analysis)
- [x] 5b.9 Performance budget — refinement must add no more than ~2× over a bare heuristic on the same workload (the C# spike measured ~1.8×)
- [x] 5b.10 Run report MUST include `enclosing_range_source` counts (scip / refinement / heuristic / dropped)
- [x] 5b.11 No `tree-sitter` or `tree-sitter-c-sharp` dependency in `kenn-indexer`'s Cargo.toml — assert via test/lint

## 5c. Symbol-kind classifier (SCIP descriptor grammar fallback)

- [x] 5c.1 Implement `classify_symbol_kind_from_descriptor(symbol_string: &str) -> SymbolKind` that parses the last descriptor of a SCIP symbol per the spec: trailing `/` → Namespace; `#` → Type; `().` → Method; `.` (after `#`-terminated parent or top-level) → Field/Property; `(name)` → Parameter; `[T]` → TypeParameter; `+` → Macro
- [x] 5c.2 Define `SymbolKindProvider` chain: tier 1 = `SymbolInformation.kind` when non-zero (scip-go, rust-analyzer); tier 2 = descriptor classifier (scip-dotnet, scip-typescript, scip-python); tier 3 = `Unknown`
- [x] 5c.3 Test: descriptor-grammar fixture covering each suffix variant (must include synthetic SCIP symbols from at least scip-dotnet, scip-typescript, scip-python output samples to ensure cross-indexer correctness)
- [x] 5c.4 Test: regression — when the same descriptor appears under different language indexers, classifier returns identical `SymbolKind` (language-agnostic invariant)
- [x] 5c.5 Run report MUST include `symbol_kind_source` counts (scip / descriptor / unknown)

## 6. Language driver: scip-dotnet

- [x] 6.1 Define `LanguageDriver` trait with `discover_units(workspace) -> Vec<Unit>`, `run(unit, workspace_cfg) -> Result<ScipFilePath>`, `language_id() -> &str`
- [x] 6.2 Implement `CSharpScipDotnet` driver
- [x] 6.3 `discover_units`: glob `**/*.sln` minus excluded paths
- [x] 6.4 `run`: spawn `scip-dotnet index <sln>`; capture stdout/stderr; parse exit + per-project failure lines
- [x] 6.5 Detect "scip-dotnet not found" (Tier 2 unavailable) and return structured `Unavailable` rather than failing
- [x] 6.6 Detect MSBuildWorkspace vulnerability-error pattern and emit a `directory_build_props_hint` in the run report
- [x] 6.7 Test: invoke the driver against the spike's the small C# sample checkout; assert run report status is `Success` and at least N documents emitted
- [x] 6.8 Test (slow / opt-in): same against the C# spike; assert wall clock < 3 minutes and run report `status = Partial` with expected MSBuild failures

## 7. IndexerDriver orchestrator

- [x] 7.1 Implement `IndexerDriver { workspace, language_drivers: Vec<Box<dyn LanguageDriver>> }`
- [x] 7.2 Method `run_all(sink: &mut dyn Sink) -> Vec<RunReport>`: for each language driver, discover units, run each unit, parse `.scip`, push records to sink
- [x] 7.3 Concurrency: serial within a language driver, parallel across language drivers (config flag, default true)
- [x] 7.4 On unit failure (process crashed, no output file): emit `RunReport { status: Failed }` and continue with remaining units
- [x] 7.5 Test: two `.sln` files in a fixture workspace; assert two RunReports, both Success, sink received records from both

## 8. Multi-run merge & collision detection

- [x] 8.1 Implement `materialize(runs: &[RunPartition]) -> MaterializedView` that dedups occurrences on `(canonical_path, symbol, range, role)` across run partitions
- [x] 8.2 During materialization, build `symbol_to_paths: HashMap<SymbolString, HashSet<canonical_path>>` (definition occurrences only)
- [x] 8.3 Emit `SymbolCollision` warning when |paths| > 1 for a given symbol; attach to the consolidated report
- [x] 8.4 Test: synthetic two-run fixture with overlapping `Host.cs` proves dedup
- [x] 8.5 Test: synthetic two-project "Common" namespace collision proves warning emitted, both records retained

## 9. ~~Run report persistence (JSONL day-1 path)~~ — **REMOVED**

Storage is SurrealDB-only; the producer streams records into a `SurrealdbSink`
(implemented in `indexed-store-and-lifecycle`). A JSONL intermediate adds disk
I/O, serialization round-trips, and a temp-dir + atomic-rename layer that
SurrealDB's transactional ingest already handles. The atomicity requirement
(crashed run leaves no partial state) becomes the storage layer's responsibility.

- [x] ~~9.1-9.3 JsonlSink + atomic commit~~ — superseded; see `indexed-store-and-lifecycle`

## 10. Configuration

- [x] 10.1 Define `kenn.toml` schema: `[workspace] root`, `[exclude] globs`, `[language.csharp] enabled, scip_dotnet_path, provision_directory_build_props (bool, default false)`
- [x] 10.2 Implement loader with sane defaults (no config file required for happy path: workspace root = cwd, default excludes, autodetect scip-dotnet on PATH)
- [x] 10.3 CLI binary `kenn index [--workspace <path>] [--config <path>]` (thin wrapper over `IndexerDriver::run_all`)
- [x] 10.4 Test: end-to-end CLI run on a small C# fixture produces `./.kenn/runs/<id>/*.jsonl`

## 11. Optional `Directory.Build.props` provisioning

- [x] 11.1 Implement `provision_csharp_directory_build_props(workspace) -> ProvisionResult` (Created | AlreadyExists | UserDeclined)
- [x] 11.2 Gate behind config flag `provision_directory_build_props = true` AND interactive prompt OR explicit CLI flag `--auto-fix-directory-build-props`
- [x] 11.3 Never modify an existing file
- [x] 11.4 Test: missing file + opt-in → file written
- [x] 11.5 Test: existing file + opt-in → file untouched, hint emitted

## 12. Documentation & developer experience

- [x] 12.1 README in `crates/kenn-indexer/` covering: how to install scip-dotnet, the Directory.Build.props caveat, sample `kenn.toml`, sample CLI invocation, expected output layout
- [x] 12.2 Architecture diagram (ASCII) in `docs/kenn/indexer-architecture.md` showing IndexerDriver → LanguageDriver → SCIP binary → parser → Sink
- [x] 12.3 Empirical-anchors note in the README: "On a 303k-LoC C# spike, expect ~90s wall clock and ~67 MB JSONL output"

## 13. Validation gate

- [x] 13.1 `openspec validate scip-indexing-pipeline --strict` passes
- [x] 13.2 All scenarios from `specs/kenn-data-model/spec.md` and `specs/scip-indexer/spec.md` have at least one test asserting them
- [x] 13.3 Manual smoke: run on a small C# fixture, inspect `./.kenn/runs/*/edges.jsonl`, confirm Clean-Architecture project rollup matches the spike's findings
