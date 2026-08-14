# scip-indexer

## Purpose

The SCIP-specific producer. Discovers indexable units in a workspace (e.g., `.sln` for C#, `Cargo.toml` for Rust, `tsconfig.json` for TypeScript), invokes the right `scip-*` binary per language out-of-band, parses the resulting `.scip` protobuf as a stream, transforms it into the `code-intel-data-model`, and reports per-unit success/partial/failed status. Handles path canonicalization, git-aware worktree exclusion, merge-time dedup, per-project failure tolerance, and a language-specific positional refinement that fills in `Occurrence.enclosing_range` (for FROM attribution) without an AST parser.
## Requirements
### Requirement: Out-of-band execution

The SCIP indexer SHALL run as an out-of-band process — invoked by an explicit command, file-watcher event, or scheduler — and MUST NOT be invoked during MCP request handling. Downstream consumers (DB ingest, MCP queries) read from previously persisted data; they never block on a live indexer run.

#### Scenario: MCP server starts mid-index

- **WHEN** an MCP session starts while a SCIP indexer run is in progress
- **THEN** the MCP server MUST serve queries from the previously persisted data without waiting for the indexer
- **AND** newly produced data from the in-progress run MUST become visible only after the run completes and is committed

### Requirement: Indexable-unit discovery

The indexer SHALL discover indexable units by scanning the workspace for files matching configured patterns per language (e.g., `**/*.sln` for C#, `Cargo.toml` for Rust, **one unit per `go.mod` module root for Go**, **workspace root for Python**). TypeScript is no longer discovered by the SCIP path — it is produced by the `typescript-stream-indexer` (`kenn-ts`) JSONL producer. The discovery rule for each language MUST be configurable. Files under configured **explicit-exclude globs** (default: `node_modules/`, `bin/`, `obj/`, `target/`) MUST be skipped, and additionally files under any **other-worktree directories** (see *Git-aware worktree exclusion*) MUST be skipped. For Python the indexer SHALL additionally skip `__pycache__/`, `.venv/`, `venv/`, and `.kenn/` (`node_modules/` is already covered by the default explicit-exclude globs). For Go the indexer SHALL additionally skip `vendor/` and `testdata/` (so a `go.mod` under vendored dependencies or test fixtures does not become its own unit).

The Python rule replaces the earlier "package roots for Python" phrasing: scip-python loads the project graph regardless of scoping, so emitting one unit at the workspace root is both correct and the minimum-work scheduling. When at least one `.py` (or `.pyi`) file is present under the workspace root (after all exclusions), the SCIP indexer MUST emit exactly one Python unit whose path is the workspace root. When no `.py`/`.pyi` file is found after exclusions, the SCIP indexer MUST emit zero Python units and MUST NOT spawn `scip-python`.

The Go rule makes the earlier "`*.go`/module roots for Go" phrasing concrete: scip-go is module-scoped (`--module-root` points at a single `go.mod`), so the indexer emits **one unit per discovered `go.mod`**, each unit's path being the directory containing that `go.mod`. When at least one `go.mod` is present under the workspace root (after all exclusions), the SCIP indexer MUST emit one Go unit per `go.mod`. When no `go.mod` is found after exclusions, the SCIP indexer MUST emit zero Go units and MUST NOT spawn `scip-go`.

#### Scenario: Multiple solutions in one workspace

- **WHEN** a workspace contains both `App.sln` and `Worker/Worker.sln`
- **THEN** the indexer MUST treat each as a distinct indexable unit
- **AND** SHALL run scip-dotnet once per unit

#### Scenario: TypeScript is not a SCIP unit

- **WHEN** a workspace contains `tsconfig.json` projects
- **THEN** the SCIP indexer MUST NOT discover them as SCIP units (they are handled by the `kenn-ts` JSONL producer)

#### Scenario: Workspace contains Python sources

- **WHEN** a workspace contains at least one `.py` file (e.g., `tmp/graphify` with 91 files under `graphify/` and `tests/`)
- **THEN** the SCIP indexer MUST emit exactly one Python unit
- **AND** the unit path MUST be the workspace root, not a sub-directory

#### Scenario: Workspace contains no Python sources

- **WHEN** a workspace has no `.py` or `.pyi` file under the root
- **THEN** the SCIP indexer MUST emit zero Python units
- **AND** `scip-python` MUST NOT be spawned

#### Scenario: Python files exist only inside excluded directories

- **WHEN** the only `.py` files under the workspace root live in `.venv/`, `__pycache__/`, `node_modules/`, or `.kenn/`
- **THEN** the SCIP indexer MUST emit zero Python units
- **AND** `scip-python` MUST NOT be spawned

#### Scenario: Workspace contains a single Go module

- **WHEN** a workspace contains exactly one `go.mod` at its root
- **THEN** the SCIP indexer MUST emit exactly one Go unit
- **AND** the unit path MUST be the directory containing `go.mod`

#### Scenario: Workspace contains multiple Go modules

- **WHEN** a workspace contains `go.mod` and `service/go.mod`
- **THEN** the SCIP indexer MUST emit two Go units, one per module root
- **AND** each unit's `--output` path MUST be distinct

#### Scenario: Workspace contains no Go module

- **WHEN** a workspace has `.go` files but no `go.mod` after exclusions
- **THEN** the SCIP indexer MUST emit zero Go units
- **AND** `scip-go` MUST NOT be spawned

#### Scenario: go.mod exists only inside excluded directories

- **WHEN** the only `go.mod` files under the workspace root live in `vendor/` or `testdata/`
- **THEN** the SCIP indexer MUST emit zero Go units
- **AND** `scip-go` MUST NOT be spawned

### Requirement: Git-aware worktree exclusion

When the workspace root is inside a git repository, the indexer SHALL query the repository for the list of linked worktrees (equivalent to `git worktree list --porcelain`) and SHALL exclude any worktree directory that is a descendant of the workspace root and is not the workspace root itself. Worktree paths are not assumed to live under any conventional name (e.g., `.worktrees/`); they are discovered from git, not from path patterns. When the workspace root is not a git repository, this rule does not apply and only explicit-exclude globs are consulted.

#### Scenario: Linked worktree under a non-conventional path

- **WHEN** the workspace root is `/repo` and `git worktree list` reports a linked worktree at `/repo/wt/feature-x`
- **THEN** files under `/repo/wt/feature-x/` MUST be excluded from indexable-unit discovery and from path canonicalization output

#### Scenario: Linked worktree under `.worktrees/`

- **WHEN** the workspace root is `/repo` and `git worktree list` reports a linked worktree at `/repo/.worktrees/feature-x`
- **THEN** files under `/repo/.worktrees/feature-x/` MUST be excluded
- **AND** the exclusion MUST be driven by the git query, not by hard-coding the `.worktrees/` path

#### Scenario: Linked worktree outside the workspace root

- **WHEN** the workspace root is `/repo` and a linked worktree exists at `/home/user/feature-x`
- **THEN** the worktree exclusion rule has no effect (the path is already outside the workspace) and MUST NOT cause an error

#### Scenario: Workspace root is itself a linked worktree

- **WHEN** the user runs the indexer with the workspace root set to a linked worktree path (e.g., `/repo/wt/feature-x`)
- **THEN** that path MUST NOT be excluded — the workspace root is always indexed, regardless of whether it is the main worktree or a linked one
- **AND** other linked worktrees that happen to be under it MUST still be excluded

#### Scenario: Workspace root is not a git repository

- **WHEN** the workspace root has no `.git` directory or file (no git repo)
- **THEN** the indexer MUST NOT attempt the git query
- **AND** only explicit-exclude globs MUST be consulted

### Requirement: Per-language indexer dispatch

The indexer driver SHALL maintain a registry mapping (language, indexable-unit-kind) to a SCIP indexer **launcher command** — a non-empty `Vec<String>` of tokens whose first element is the program subject to the Tier-2 availability probe and whose remaining elements are leading arguments prepended to the driver's intrinsic arg list (per the *Driver launcher is a token vector across all SCIP and JSONL languages* requirement). Per-language defaults: C# → `["kenn-dotnet"]` (JSONL producer, not a `scip-*` binary), Python → `["scip-python"]` (then `index ...`), Go → `["scip-go"]` (then `index ...`), Rust → `["rust-analyzer"]` (then `scip ...`). TypeScript SHALL NOT have a SCIP registry entry — it is produced by the `kenn-ts` JSONL producer, not a `scip-*` binary. The registry MUST be extensible without code changes when reasonable (config-driven via `[language.*]` blocks).

This requirement replaces the earlier single-string phrasing ("the registry maps ... to a SCIP indexer command. For C#: `scip-dotnet index <sln>`"). The launcher-command shape lets users invoke a binary directly, or through a package runner (`bunx`, `npx`, `uvx`) — the registry just records tokens, kenn does not interpret them.

#### Scenario: Adding a new language indexer

- **WHEN** a new entry is registered mapping `(language="kotlin", unit=".gradle.kts")` to launcher `["scip-kotlin"]`
- **THEN** the indexer driver MUST pick up Kotlin units in subsequent runs without code changes

#### Scenario: TypeScript has no SCIP command

- **WHEN** the SCIP driver registry is consulted for `language="typescript"`
- **THEN** there is no entry (TypeScript indexing is the `kenn-ts` JSONL producer's responsibility)

#### Scenario: Python registry entry is a launcher vector, not a single string

- **WHEN** the user configures `[language.python] command = ["bunx", "@sourcegraph/scip-python"]`
- **THEN** the registry MUST record the full token vector and the driver MUST invoke it verbatim
- **AND** the Tier-2 probe MUST target `bunx` (per the launcher-vector requirement)

### Requirement: Tier-2 availability detection

For each language enabled in configuration, the indexer driver SHALL probe whether the corresponding SCIP binary (the first token of the configured `command` vector) is available and runnable. If unavailable, the run SHALL fail in the prepare phase — before any store write — with a clear `<language>: required command \`<token>\` not found on PATH` message, per the *indexing-orchestrator preflight* contract. The run SHALL NOT proceed to ingest with the failing language silently skipped.

This requirement replaces the earlier "continue with other languages" phrasing. The actual `preflight()` implementation hard-fails the run when any configured CLI is missing, and `indexing-orchestrator`'s prepare-phase requirement mandates exactly that. The earlier wording was a design aspiration that never matched the code; aligning the spec eliminates the conflict.

#### Scenario: Configured C# launcher missing

- **WHEN** the SCIP driver runs in a workspace with C# projects but the configured C# `command` first token is not on PATH
- **THEN** the run MUST fail in the prepare phase with a clear `csharp: required command \`<token>\` not found on PATH` message
- **AND** no store write MUST have occurred

#### Scenario: scip-python launcher missing

- **WHEN** Python is enabled with `command = ["bunx", "@sourcegraph/scip-python"]` and `bunx` is missing from PATH
- **THEN** the run MUST fail in the prepare phase with `python: required command \`bunx\` not found on PATH`
- **AND** no store write MUST have occurred — other enabled languages MUST NOT have started ingesting either

### Requirement: Enclosing-range provider for FROM attribution

The driver SHALL provide an `enclosing_range` for every non-definition occurrence so that derived edges (calls / has-a / uses-a) can be attributed to the correct enclosing symbol per `kenn-data-model`. The provider SHALL use a three-tier fallback in order:

1. The `Occurrence.enclosing_range` field from SCIP, when non-empty.
2. A language-specific positional refinement that reads the source file (no AST parser) and corrects systematic errors of the bare last-preceding-def heuristic. For C# this MUST include:
   - **Parameter-kind exclusion** — defs whose SCIP descriptor leaf is `(name)` (parameter symbols) MUST be excluded from FROM candidates; references inside a method body belong to the method, not to one of its parameters.
   - **Attribute-line re-anchor** — a non-def occurrence whose source line is part of an attribute list (line begins with `[` and is not in expression-continuation context) MUST have its effective position advanced to the next code line before the heuristic runs, so the attribute's target declaration becomes the FROM. The classifier MUST distinguish C# 12 collection literals (`x = [a, b, c]`) from attribute lists by checking whether the previous non-blank, non-comment line ends in an expression-continuation token (`=`, `=>`, `,`, `(`).
   - **Same-line forward-def lookup** — when the occurrence sits on the same line as a forthcoming def's identifier (e.g., `public BigDecimal AveragePrice { get; set; }` — `BigDecimal` precedes `AveragePrice`'s identifier column), the FROM MUST be the def on that line whose identifier column is the smallest one ≥ the occurrence column.
3. A last-preceding-def positional heuristic, only when (1) and (2) leave no FROM (source file missing on disk, classifier produced no candidate).

The driver SHALL NOT use tree-sitter or any other AST parser for FROM attribution.

#### Scenario: SCIP populates enclosing_range

- **WHEN** an `Occurrence` from a SCIP indexer carries a non-empty `enclosing_range`
- **THEN** the driver MUST use that range and skip the positional refinement for this occurrence

#### Scenario: C# attribute decorating a property

- **WHEN** the source contains `[DataMember]\n    public string Name { get; set; }` and a non-def occurrence on the `[DataMember]` line refers to `DataMemberAttribute`
- **THEN** the driver MUST attribute the resulting edge's FROM to the property `Name`, not to the previously-declared sibling

#### Scenario: C# 12 collection literal not confused with attribute list

- **WHEN** the source contains `public static readonly Foo[] Active =\n    [\n        Foo.Pending,\n        Foo.Submitted\n    ];` and a non-def occurrence on the `Foo.Pending` line refers to `Foo.Pending`
- **THEN** the driver MUST attribute the FROM to `Active` (the field whose initializer this is), not to a later sibling field — the leading `[` on the line above MUST NOT be misclassified as an attribute list because the previous line ends with `=`

#### Scenario: Reference inside a method body, not its parameter

- **WHEN** a non-def occurrence inside a method body refers to some symbol AND the method declares a parameter `(p)` whose def position precedes the occurrence
- **THEN** the parameter MUST NOT be selected as FROM — the method itself is the FROM

#### Scenario: Type reference on the same line as the property's identifier

- **WHEN** the source contains `public BigDecimal AveragePrice { get; set; }` and a non-def occurrence at the `BigDecimal` column refers to `BigDecimal`
- **THEN** the driver MUST attribute the FROM to `AveragePrice`, not to the previously-declared sibling

#### Scenario: Source file missing on disk

- **WHEN** SCIP `enclosing_range` is empty AND the source file referenced by `canonical_path` is not readable
- **THEN** the driver MAY fall back to the bare last-preceding-def positional heuristic, OR drop the derived edge — implementation-defined, but the run report MUST record the count of occurrences that hit this fallback path

### Requirement: Output parsing

The indexer driver SHALL parse each SCIP indexer's `index.scip` output as a protobuf `Index` message and transform it into the `kenn-data-model` representation. The parser MUST stream the protobuf rather than loading the entire file into memory, to handle individual documents up to several megabytes in size.

#### Scenario: Large single-document file

- **WHEN** an `index.scip` file contains a single `Document` of size 4 MB (e.g., a generated EF migration)
- **THEN** the parser MUST process it without loading more than one document's worth of memory at a time

### Requirement: def_range Is Populated for Every Non-External Symbol

For every symbol emitted into the `code-intel-data-model` from a SCIP source, the transform SHALL populate `DefRecord.{start_line, start_col, end_line, end_col}` from the `Occurrence` whose `symbol_roles` includes `SymbolRole::Definition` for that symbol in the indexed documents. The transform MUST NOT push a placeholder `[0, 0, 0, 0]` `DefRecord` for symbols that have a definition occurrence in the SCIP file.

Synthetic and external symbols (those without a `Definition` occurrence in any indexed document — typically symbols declared in dependencies the SCIP file references but does not index) MAY have `def_range = [0, 0, 0, 0]`; in that case the symbol MUST also be marked `is_external = true`.

Stored lines are 1-based per the `source-data-model` requirement; the conversion happens during this transform.

#### Scenario: A Rust function declared at file line 10 has non-zero def_range

- **WHEN** a Rust function is indexed and its SCIP `Occurrence` has `symbol_roles & Definition != 0` with `range = [9, 4, 9, 24]` (0-based)
- **THEN** the resulting `DefRecord` MUST have `start_line = 10, end_line = 10`
- **AND** the `defs` row MUST NOT be `[0, 0, 0, 0]`

#### Scenario: An external symbol with no Definition occurrence keeps zero range

- **WHEN** a symbol appears only as a `Reference` (e.g., `std::vec::Vec` used but not defined in the indexed Cargo unit)
- **THEN** the `DefRecord` MAY be `[0, 0, 0, 0]`
- **AND** the symbol MUST be marked `is_external = true`

#### Scenario: A symbol with multiple Definition occurrences (partial / cfg-gated)

- **WHEN** a Rust item has two `Definition` occurrences (e.g., two `#[cfg(...)]`-gated `impl` blocks)
- **THEN** the `defs` table MUST contain one row per `Definition` occurrence
- **AND** all rows MUST share the same `sym_id` with distinct `file_id`/`start_line`

### Requirement: Path canonicalization at ingest

Before producing data-model records, the indexer driver SHALL canonicalize each document path using `metadata.project_root + relative_path` → absolute → workspace-relative. Records whose canonicalized path falls outside the configured workspace root MUST be dropped with a warning.

#### Scenario: Worker.sln rooted under some-repo/Worker

- **WHEN** scip-dotnet emits `metadata.project_root = file:///some-repo/Worker` and a document `relative_path = "Host/Host.cs"`
- **AND** the workspace root is `/repo`
- **THEN** the canonicalized path MUST be `Worker/Host/Host.cs`

### Requirement: Multi-run merge

When multiple indexable units cover overlapping source files (e.g., two `.sln` files share projects), the indexer driver SHALL merge their outputs by deduping on `(canonical_path, symbol, range)` per the data-model contract. Records that differ only by indexer-run provenance MUST be retained per-run; the dedup applies only to identical `(path, symbol, range, role)` tuples within a single materialized view.

#### Scenario: Same file in two solutions

- **WHEN** `Worker/Host/Host.cs` is indexed by both `App.sln` and `Worker/Worker.sln` runs
- **THEN** the materialized data MUST contain each definition once, not twice
- **AND** each occurrence record MUST retain provenance from at least one run

### Requirement: Per-unit failure tolerance

If indexing of a single project within an indexable unit fails (compile error, vulnerability error, missing reference, SDK mismatch), the driver MUST NOT abort the entire run. The driver SHALL collect per-project failure diagnostics and report them alongside successful output. Partial coverage is an acceptable result.

#### Scenario: One project in a solution fails to load

- **WHEN** a `.sln` contains 100 projects and 3 fail with `MissingMethodException` from MSBuild
- **THEN** the run MUST emit data-model records for the 97 successful projects
- **AND** the run report MUST list the 3 failed projects with their error messages

### Requirement: C# workspace prerequisites

For C# (`scip-dotnet`), the indexer driver SHALL document and (optionally) provision a `Directory.Build.props` recipe at the workspace root with `<NuGetAudit>false</NuGetAudit>` to prevent MSBuildWorkspace from blocking on transitive package vulnerability errors. Provisioning MUST be opt-in and MUST NOT modify an existing user-managed `Directory.Build.props` without explicit consent.

#### Scenario: User has no Directory.Build.props

- **WHEN** the driver runs in a C# workspace lacking `Directory.Build.props` and the user has opted in to provisioning
- **THEN** the driver MUST create one containing `<NuGetAudit>false</NuGetAudit>`

#### Scenario: User already has Directory.Build.props

- **WHEN** the driver runs in a C# workspace that already has a `Directory.Build.props`
- **THEN** the driver MUST NOT modify the file
- **AND** SHALL emit a hint instructing the user to add `<NuGetAudit>false</NuGetAudit>` if they hit vulnerability errors

### Requirement: Run report

The system MUST persist a per-run report containing counts of
documents, symbols, occurrences, and relationships processed, a
list of failed projects with diagnostics, and a status field of
`success | partial | failed`. The report MUST be persisted
alongside the produced data-model records.

The `documents` count SHALL equal the number of distinct source
file paths the indexer visited in this run — not zero, not the
number of `FileRecord` rows emitted to the `files` table.
A path that appears in multiple SCIP `Document` messages (e.g. one
per project in a multi-csproj solution) MUST be counted exactly
once.

#### Scenario: Querying the latest run for a unit

- **WHEN** a consumer asks for the latest run report for `App.sln`
- **THEN** the system MUST return the most recent report including its status, counts, and any failures

#### Scenario: documents count is non-zero on a non-empty workspace

- **WHEN** `kenn index` runs against a workspace containing N
  source files of an enabled language and the run completes
  successfully
- **THEN** `meta.json["documents"]` SHALL be ≥ 1
- **AND** `meta.json["documents"]` SHALL equal the number of
  distinct source file paths the indexer visited (deduplicated
  across SCIP `Document` messages that repeat a path)

#### Scenario: documents count survives intern dedup

- **WHEN** the SCIP stream contains two `Document` messages with
  the same `relative_path` (e.g. the same file emitted from two
  csproj projects)
- **THEN** the `files` table MUST contain exactly one
  `FileRecord` for that path (existing intern-dedup behaviour
  preserved)
- **AND** `meta.json["documents"]` MUST count that path once,
  regardless of whether the second `Document` produced a
  `FileRecord` (i.e., the dedup gate on `FileRecord` emission
  MUST NOT silently suppress the file-count increment)

### Requirement: Streaming bulk emission

The indexer driver SHALL emit data-model records as a stream/iterator, not a single in-memory collection, so a downstream DB-ingest layer can perform bulk-insert in batches. The streaming contract MUST allow back-pressure (the consumer can pause).

#### Scenario: Slow consumer

- **WHEN** the DB-ingest layer cannot accept new records as fast as the parser produces them
- **THEN** the parser MUST block (or yield) rather than buffer indefinitely

### Requirement: Symbol id collision detection

When merging runs, the driver SHALL detect cases where the same `symbol_string` is emitted with definitions in two distinct canonical paths (the project-disambiguation collision documented in the spike). Each such case MUST produce a structured warning entry in the run report identifying the symbol and the two paths, but MUST NOT fail the run.

#### Scenario: Two "Common.Helpers" classes in different projects

- **WHEN** project `some-repo/Common` and project `some-repo/Worker/Common` both define a class with identical scip symbol_string and the data model retains both per the dedup rule
- **THEN** the run report MUST contain a `symbol_collision` warning naming both canonical paths

The "aggregated graph" referenced below is the weighted undirected graph
defined in the `graph-analysis` capability: a projection of the per-symbol
graph in which each method, field, free function, parameter, etc. is
rolled up to its nearest enclosing class-like or module-like symbol, and
edges of the kept kinds (`calls`, `type_use`, `field_access`, `implements`,
`instantiates`, `overrides`, plus module-to-module `imports`) are
aggregated as undirected weighted edges between those anchor symbols.
See `specs/graph-analysis/spec.md` for the full roll-up rules, kept kinds,
and per-kind weights.

### Requirement: Aggregate-graph computation during end_run

The indexer pipeline SHALL compute the aggregated graph as a step inside `end_run`, after every per-unit transform has flushed its symbol, edge, and def records, and before snapshot publication. Both the SCIP transform path and the JSONL transform path feed into the same aggregation step — aggregation reads the already-persisted symbol and edge tables to perform the roll-up rather than re-deriving aggregates per document.

The aggregation step SHALL:

1. Build an in-memory `HashMap<ShortId, SymbolRow>` by streaming the symbol table.
2. Compute `aggregate_id` for each symbol by walking the `enclosing_symbol` chain to the nearest class-like or module-like symbol (cycle-safe; falls back to self when no anchor is found).
3. Stream every persisted edge, look up the aggregates for both endpoints, drop self-loops on the aggregate graph, drop kinds not in the kept-kinds set, and accumulate weights into a `HashMap<(min_agg, max_agg, EdgeKind), u32>`.
4. Resolve each aggregate node's anchor via `pkg` → file-path prefix → `<unanchored>`.
5. Persist the resulting nodes and edges to the new `aggregate_nodes` / `aggregate_edges` tables.

#### Scenario: Aggregation runs once per index, not per document

- **WHEN** a workspace with N source documents is indexed
- **THEN** the aggregation step MUST execute exactly once per `kenn index` invocation, after all N documents are ingested

#### Scenario: Aggregation reads from persisted tables, not from in-flight buffers

- **WHEN** the aggregation step begins
- **THEN** it MUST source symbols and edges from the snapshot's persisted tables (via the same `scan_*` paths the analyzer uses)
- **AND** it MUST NOT depend on transform-time per-document state

### Requirement: Aggregation cost budget

The aggregation step SHALL be O(N + E) in the number of symbols and persisted edges. On a typical workspace its contribution to total `kenn index` wall-time SHALL be under 10%. Compliance is measured via the existing `KENN_BENCH` instrumentation by adding a `BENCH end_run: aggregate=<ms>` line to the pipeline output.

#### Scenario: Bench output reports aggregate timing

- **WHEN** `KENN_BENCH=1 kenn index` runs against any workspace
- **THEN** the bench output MUST include a line of the form `BENCH end_run: aggregate=<integer>ms`

### Requirement: Aggregation determinism

The aggregation step SHALL be deterministic: indexing the same source state twice MUST produce byte-identical `aggregate_nodes` and `aggregate_edges` tables. Iteration orders MUST be sorted (symbols by `short_id` ascending; edges by `(min_agg, max_agg, kind)`).

#### Scenario: Repeated index produces identical aggregate tables

- **WHEN** `kenn index --force` runs twice in succession on an unchanged workspace
- **THEN** both runs MUST produce snapshots whose `aggregate_nodes` and `aggregate_edges` tables, when scanned, return identical byte sequences

### Requirement: Aggregation tolerates incomplete ingest

When `kenn index` reports a `partial` status (at least one unit failed), aggregation SHALL still run on the symbols and edges that did ingest successfully. The snapshot SHALL be published as `partial` with non-empty aggregate tables reflecting whatever was ingested.

#### Scenario: Partial ingest still produces aggregated graph

- **WHEN** one of three configured C# projects fails during ingest but the other two succeed
- **THEN** the published snapshot's `aggregate_nodes` and `aggregate_edges` tables MUST contain the rolled-up graph of the two successful projects
- **AND** the snapshot status MUST be `partial`

### Requirement: User→external edges are emitted

The SCIP edge-derivation pass SHALL emit edges whose target has zero workspace definitions (`def_count == 0` in the per-run def-count map). The target SHALL be interned via the stub path so it appears in the symbols table. The `def_count > 1` arm SHALL continue to drop occurrences — that filter targets crate-root markers and known producer duplication patterns, and its relaxation is deferred to a separate change with its own evidence base.

#### Scenario: Stdlib reference reaches the graph

- **WHEN** a SCIP `Occurrence` references a target with `def_count == 0` (e.g. `Result::unwrap`)
- **THEN** the edge SHALL be emitted with the enclosing workspace symbol as source and the external symbol as target
- **AND** the target SHALL be interned via the stub path so it appears in the symbols table

#### Scenario: Ambiguous-target reference is still dropped

- **WHEN** an `Occurrence` references a target with `def_count > 1` (e.g. a crate-root marker emitted from multiple files)
- **THEN** the occurrence SHALL be dropped

### Requirement: Drained stubs are tagged external

`flush_registry_stubs` SHALL set `is_external = true` on every `SymbolRecord` it pushes to the sink. A drained stub is by construction a symbol whose full `SymbolFrame` (carrying its definition) never arrived during ingest; such symbols are defined outside the workspace. This holds for both the SCIP and JSONL ingest paths — the JSONL path's existing `pkg_external` plumbing already tags *full* symbols correctly; drain-time tagging closes the stub-only gap on both paths.

#### Scenario: Stdlib symbol drained as external

- **WHEN** the SCIP edge-derivation pass interns a stub for a stdlib symbol (e.g. `core::result::Result::unwrap`) and no document in the run provides a full `SymbolFrame` for it
- **THEN** `flush_registry_stubs` SHALL emit that stub's `SymbolRecord` with `is_external = true`

#### Scenario: Cross-document workspace symbol promoted before drain

- **WHEN** a stub is buffered for a workspace symbol referenced from a document that doesn't define it, and a later document in the same run provides the defining `SymbolFrame`
- **THEN** `mark_full_emitted` SHALL remove the stub from the pending map before drain
- **AND** the symbol SHALL appear in the symbols table with `is_external = false` (from the full record path)

#### Scenario: include_external filter affects SCIP-language results

- **WHEN** an MCP query passes `include_external: false` to `find_symbol` / `search_symbols` / `list_callers` over a Rust workspace
- **THEN** the returned rows SHALL exclude symbols with `is_external = true`
- **AND** the filter SHALL produce results equivalent to the prior behavior (when external edges did not exist in the graph), modulo the absence of any external edges or symbols from the result set

### Requirement: Python indexer dispatch via launcher command

When at least one Python unit has been discovered, the indexer driver SHALL invoke `scip-python` once per unit by spawning the configured launcher command (a non-empty sequence of tokens), passing `index --cwd <workspace-root> --output <run-dir>/python-<idx>.scip --quiet` as the trailing arguments where `<idx>` is the unit's 0-based discovery index and `<run-dir>` is the active indexer-pass run directory under the derived store. When the unit's path is a strict descendant of the workspace root (i.e., `[language.python].targets` is non-empty and named that path), the driver MUST additionally pass `--target-only <unit-path>` as a trailing argument. When the configured `project_name` or `project_version` is set, the driver MUST forward each as `--project-name <name>` and `--project-version <version>` respectively for every per-unit invocation. The produced `.scip` outputs SHALL be ingested through the existing SCIP output-parsing requirement and the `PythonTransformer` rewrites `scip-python python <dist> <ver> <descriptor>` symbols into `py:<module>.<...>` public IDs.

This requirement replaces the earlier single-invocation phrasing. The per-target-invocation shape supports monorepo workspaces with multiple Python sub-packages, each invoked with its own `--target-only` for narrowed `TreeVisitor` walks. Pyright analysis state is NOT shared across invocations (scip-python's `Indexer` is a per-process construct); the cost is N × per-target analysis when `targets` has N entries.

#### Scenario: Default launcher with no targets configured

- **WHEN** the user configures `[language.python] enabled = true` with empty `targets`
- **THEN** the driver MUST spawn one `scip-python index --cwd <ws> --output <run-dir>/python-0.scip --quiet` invocation
- **AND** MUST NOT pass `--target-only`

#### Scenario: Single target

- **WHEN** the user configures `targets = ["src/api"]`
- **THEN** the driver MUST spawn one invocation with `--target-only <ws>/src/api` appended

#### Scenario: Multiple targets fan out to N invocations

- **WHEN** the user configures `targets = ["src/api", "src/worker"]`
- **THEN** the driver MUST spawn two `scip-python` invocations, one per target
- **AND** each invocation's `--output` path MUST be distinct (e.g., `python-0.scip` and `python-1.scip`)
- **AND** each invocation MUST include `--target-only` pointing at its target's resolved path

#### Scenario: project_name and project_version forwarded to every invocation

- **WHEN** `project_name = "monorepo"` is set and `targets = ["src/api", "src/worker"]`
- **THEN** both spawned invocations MUST include `--project-name monorepo`

#### Scenario: Launcher routes through bunx (single target)

- **WHEN** the user configures `command = ["bunx", "@sourcegraph/scip-python"]` and `targets = ["src/api"]`
- **THEN** the driver MUST spawn `bunx @sourcegraph/scip-python index --cwd <ws> --output <run-dir>/python-0.scip --quiet --target-only <ws>/src/api`

#### Scenario: Python symbols become py: public IDs

- **WHEN** `scip-python` emits a symbol `scip-python python graphify 0.8.12 graphify/detect/detect_languages().`
- **THEN** the resulting record's public ID MUST be `py:graphify.detect.detect_languages`

### Requirement: Go indexer dispatch via launcher command

When at least one Go unit has been discovered, the indexer driver SHALL invoke `scip-go` once per unit by spawning the configured launcher command (a non-empty sequence of tokens), passing `index --module-root <unit-path> --output <run-dir>/go-<idx>.scip --quiet` as the trailing arguments where `<unit-path>` is the directory containing that unit's `go.mod`, `<idx>` is the unit's 0-based discovery index, and `<run-dir>` is the active indexer-pass run directory under the derived store. The produced `.scip` outputs SHALL be ingested through the existing SCIP output-parsing requirement, and the `GoTransformer` rewrites `scip-go gomod <pkg> <ver> <descriptor>` symbols into `go:<package-path>.<...>` public IDs.

scip-go shells to `go list` / `go/packages` to load the module graph, so each invocation requires the module to be buildable with its dependencies available. kenn does NOT run `go mod download` or otherwise build the module on the user's behalf — the toolchain and dependency state are the caller's responsibility, the same posture as the Rust (`rust-analyzer`) and Swift drivers. When `scip-go` exits non-zero (e.g., a missing toolchain or unresolved dependency), the unit MUST be reported as failed/unavailable rather than silently skipped.

The launcher's first token is the program subject to the Tier-2 availability probe (per *Tier-2 availability detection* and *Driver launcher is a token vector across all SCIP and JSONL languages*); the default launcher is `["scip-go"]`.

#### Scenario: Default launcher, single module

- **WHEN** the user configures `[language.go] enabled = true` and the workspace has one `go.mod` at the root
- **THEN** the driver MUST spawn one `scip-go index --module-root <ws> --output <run-dir>/go-0.scip --quiet` invocation

#### Scenario: Multiple modules fan out to N invocations

- **WHEN** the workspace has `go.mod` and `service/go.mod`
- **THEN** the driver MUST spawn two `scip-go` invocations, one per module root
- **AND** each invocation's `--module-root` MUST point at its module's directory
- **AND** each invocation's `--output` path MUST be distinct (e.g., `go-0.scip` and `go-1.scip`)

#### Scenario: Launcher routes through an absolute path

- **WHEN** the user configures `command = ["/opt/go/bin/scip-go"]` for a single-module workspace
- **THEN** the driver MUST spawn `/opt/go/bin/scip-go index --module-root <ws> --output <run-dir>/go-0.scip --quiet`

#### Scenario: scip-go launcher missing

- **WHEN** Go is enabled with `command = ["scip-go"]` and `scip-go` is not on PATH
- **THEN** the run MUST fail in the prepare phase with `go: required command \`scip-go\` not found on PATH`
- **AND** no store write MUST have occurred — other enabled languages MUST NOT have started ingesting either

#### Scenario: Go symbols become go: public IDs

- **WHEN** `scip-go` emits a symbol `scip-go gomod github.com/foo/quinn-proto 0.1.0 \`github.com/foo/quinn-proto/connection\`/Connection#New().` (the first descriptor namespace is the full package import path; the module field is separate metadata)
- **THEN** the resulting record's public ID MUST be `go:github.com/foo/quinn-proto/connection.Connection.New`
- **AND** the public ID MUST derive from the descriptor's package path, NOT by prepending the module field (which would duplicate the package path)

### Requirement: Driver launcher is a token vector across all SCIP and JSONL languages

Every SCIP driver and JSONL indexer (C#, Rust, TypeScript, Python) SHALL accept its invocation as a non-empty `command: Vec<String>` of launcher tokens — `command[0]` is the program subject to the Tier-2 availability probe (per the modified *Tier-2 availability detection* requirement), and `command[1..]` are leading arguments prepended to the driver's intrinsic arg list. Drivers MUST NOT carry a separate `binary_path: Option<PathBuf>` field; the single launcher vector subsumes both "binary on PATH" and "wrapper / package-runner" cases.

Defaults: `["kenn-dotnet"]` for C#, `["rust-analyzer"]` for Rust, `["kenn-ts"]` for TypeScript, `["scip-python"]` for Python.

#### Scenario: Tier-2 probe targets the launcher's first token

- **WHEN** any driver is invoked with `command = ["wrapper-program", "package-or-arg", ...]`
- **THEN** the probe MUST check `wrapper-program` on PATH (the package/argument tokens are not probe targets)

#### Scenario: Empty command is rejected at config load

- **WHEN** the user configures `command = []` for any language
- **THEN** config validation MUST reject the file with an error naming the offending language

### Requirement: kenn honors launcher tokens verbatim with no runtime preference

For every language driver, kenn SHALL invoke the configured `command` tokens verbatim with no auto-detection of runtimes, no fallback between runtimes, and no kenn-side preference for any specific runtime (bun, npm, pip, system PATH, or otherwise). Runtime selection is operator policy expressed through `command`; encoding a kenn-side default beyond the per-language `["<binary>"]` plain-PATH lookup would push that policy into the tool.

#### Scenario: Python launcher honored without runtime fallback

- **WHEN** the user configures `[language.python] command = ["bunx", "@sourcegraph/scip-python"]` and `bunx` is missing from PATH
- **THEN** the run MUST fail per the Tier-2-probe rule
- **AND** kenn MUST NOT attempt `npx`, `uvx`, `pip`, or a bare `scip-python` as a fallback

#### Scenario: Operator picks any runtime for any language

- **WHEN** the user configures any of `["scip-python"]`, `["bunx", "@sourcegraph/scip-python"]`, `["npx", "--yes", "@sourcegraph/scip-python"]`, `["uvx", "scip-python"]`, `["rust-analyzer"]`, `["asdf", "exec", "rust-analyzer"]`
- **THEN** kenn MUST honor the tokens verbatim — no rewriting, reordering, or substitution

#### Scenario: Rule applies to every language, not just Python

- **WHEN** any C#, Rust, or TypeScript driver is invoked with a non-default `command`
- **THEN** the same verbatim-honored, no-fallback rule MUST apply

### Requirement: All languages are opt-in by default

Every `[language.*]` block (`csharp`, `rust`, `typescript`, `python`) SHALL default `enabled = false`. The indexer MUST NOT spawn any language driver unless the user has explicitly set `enabled = true` for that language in `kenn.toml`. This applies uniformly — C# is not privileged. When no language is enabled, `kenn index` MUST complete successfully with an empty snapshot.

#### Scenario: Fresh workspace with default config indexes nothing

- **WHEN** the user runs `kenn index` against a workspace where `kenn.toml` does not enable any language
- **THEN** no driver subprocess MUST be spawned
- **AND** the run MUST complete successfully producing an empty snapshot (`documents=0 symbols=0`)

#### Scenario: C# requires explicit enable

- **WHEN** a workspace contains `.sln` / `.csproj` files but `[language.csharp].enabled` is not set
- **THEN** `kenn-dotnet` MUST NOT be spawned (C# is opt-in like every other language)

#### Scenario: Python enabled in isolation runs only scip-python

- **WHEN** `[language.python].enabled = true` and no other language is enabled
- **THEN** only the Python driver MUST run; `kenn-dotnet`, `rust-analyzer`, and `kenn-ts` MUST NOT be spawned

### Requirement: Python multi-target unit discovery

This requirement extends the cross-language *Indexable-unit discovery* requirement with Python-specific multi-target behaviour. The SCIP indexer SHALL discover Python indexable units as follows:

1. If `[language.python].targets` is empty AND at least one `.py` (or `.pyi`) file is present under the workspace root (subject to the existing explicit-exclude globs, git-aware worktree exclusion, and the Python-specific skip set `__pycache__/`, `.venv/`, `venv/`, `.kenn/`), emit exactly ONE unit whose path is the workspace root.
2. If `[language.python].targets` is non-empty, emit ONE unit per entry in the list. Each unit's path SHALL be the workspace-relative target path joined with the workspace root. The Python file existence probe is NOT performed per-target — the user's explicit target list overrides discovery.
3. If `targets` is empty AND no `.py`/`.pyi` file is found after exclusions, emit ZERO units and MUST NOT spawn `scip-python`.

Target paths in `targets` MUST be relative paths interpreted against the workspace root; absolute paths and duplicate entries SHALL be rejected at config load with an error that names the offending entry. Each resolved target path MUST exist as a directory on disk; non-existent targets MUST cause the run to fail in the prepare phase (analogous to the existing `KennDotnet::resolve_projects` behaviour for missing `.sln` paths).

#### Scenario: Empty targets with Python files present

- **WHEN** `targets = []` and the workspace contains `.py` files
- **THEN** the SCIP indexer MUST emit exactly one Python unit at the workspace root

#### Scenario: Empty targets with no Python files

- **WHEN** `targets = []` and the workspace contains no `.py`/`.pyi` files
- **THEN** the SCIP indexer MUST emit zero Python units and MUST NOT spawn `scip-python`

#### Scenario: Targets list bypasses the file existence probe

- **WHEN** `targets = ["src/empty-pkg"]` and the workspace contains `.py` files elsewhere but `src/empty-pkg` itself contains none
- **THEN** the SCIP indexer MUST still emit one unit for `src/empty-pkg`
- **AND** the resulting scip-python invocation MAY produce a near-empty `.scip` — that is the user's explicit choice

#### Scenario: Non-existent target path fails the run

- **WHEN** `targets = ["src/missing"]` and `src/missing` does not exist as a directory
- **THEN** the run MUST fail in the prepare phase with a clear error naming `src/missing`
- **AND** no store write MUST have occurred

#### Scenario: Absolute path in targets rejected at config load

- **WHEN** the user configures `targets = ["/abs/path"]`
- **THEN** config validation MUST reject the file with an error naming `python.targets[0]`

#### Scenario: Duplicate target entries rejected at config load

- **WHEN** the user configures `targets = ["src", "src"]`
- **THEN** config validation MUST reject the file with an error naming the duplicate
- **AND** the run MUST NOT spawn any scip-python invocation

### Requirement: Workspace-relative glob filter at Python ingest

The SCIP→record transform for Python SHALL consult `[language.python].exclude_documents` (a list of workspace-relative glob patterns; default `[]`) before emitting any record from each `scip.Document`. When the list is non-empty, every `Document` whose `relative_path` matches at least one pattern MUST be dropped: no `SymbolRecord`, no `DefRecord`, no occurrence-derived edge is emitted from that document.

Globs are matched against `Document.relative_path` directly using the standard glob crate semantics (`*` non-`/`, `**` recursive). No filesystem normalisation or canonicalisation is performed — scip-python emits `relative_path` as workspace-relative for in-workspace files, which is exactly what the user names in the pattern.

External `SymbolInformation` records emitted by scip-python in its dedicated `scip.Index.external_symbols` frame are NOT affected by this filter — they continue to be ingested through the existing external-symbol path so that in-workspace occurrences referencing symbols defined inside a dropped document still produce edges to external stubs (`is_external = true`).

This filter is independent of and composes with `targets`: `targets` narrows what scip-python's `TreeVisitor` walks (saves scip-python compute); `exclude_documents` narrows what kenn ingests (filters noise without affecting scip-python). Users with sub-directories that `--target-only` cannot exclude (e.g., a `node_modules/` or `__pycache__/` inside a target directory) typically pair the two: `targets = ["src"]` plus `exclude_documents = ["**/node_modules/**"]`.

#### Scenario: Document matching one pattern dropped

- **WHEN** `exclude_documents = ["worked/**"]` AND scip-python emits a `Document` with `relative_path = "worked/httpx/raw/transport.py"`
- **THEN** the SCIP transform MUST NOT emit any `SymbolRecord`, `DefRecord`, or occurrence record from that document
- **AND** the snapshot MUST NOT contain symbols whose definition lives in that document

#### Scenario: Document matching no patterns ingested

- **WHEN** `exclude_documents = ["worked/**"]` AND scip-python emits a `Document` with `relative_path = "graphify/detect.py"`
- **THEN** the SCIP transform MUST ingest every record from that document per the existing requirements

#### Scenario: Multiple patterns, OR-semantics

- **WHEN** `exclude_documents = ["worked/**", "tests/fixtures/**"]`
- **THEN** a `Document` with `relative_path = "tests/fixtures/sample.py"` MUST be dropped
- **AND** a `Document` with `relative_path = "tests/test_detect.py"` MUST be ingested (it matches neither pattern)

#### Scenario: Cross-document edge to a dropped-document symbol still emitted

- **WHEN** `exclude_documents = ["worked/**"]` AND an in-workspace document `graphify/_client.py` has a `ReadAccess` occurrence on a symbol whose Definition lives in the dropped `worked/httpx/raw/transport.py`
- **AND** scip-python's `external_symbols` frame contains the matching `SymbolInformation` for that symbol
- **THEN** the edge from `_client.py` to the symbol stub MUST still be emitted (via the existing external-symbol path)
- **AND** the resulting stub MUST be marked `is_external = true`

#### Scenario: Empty exclude_documents = current ingest behaviour

- **WHEN** `exclude_documents = []` (default)
- **THEN** every `Document` from every per-target `.scip` MUST be ingested per the existing requirements
- **AND** the snapshot's symbol count MUST be identical to the pre-flag behaviour from `kenn-python-support`

#### Scenario: Pattern composes with multi-target

- **WHEN** `targets = ["src/api", "src/worker"]` AND `exclude_documents = ["**/fixtures/**"]`
- **THEN** the filter MUST be applied uniformly to documents from both per-target `.scip` outputs

### Requirement: Python test-marking heuristics

The SCIP→record transform for Python SHALL extend `is_test_descriptor(Language::Python, kind, public_id)` to return `true` when ANY of the following holds on the public_id's native (`py:` prefix stripped, then split on `.`) dotted segments. Several rules carry a leaf/non-leaf distinction to avoid false positives on production identifiers — the Rust arm of `is_test_descriptor` uses the same pattern (`transform/naming.rs`) for the analogous reason.

1. **Test-directory segment match**: any segment is exactly one of `tests`, `test`, or `__tests__`. When the matching segment is **non-leaf** (i.e., another segment follows it), the rule fires unconditionally. When the matching segment is the **leaf**, the rule fires only when `kind.is_scope()` (Package / Module / Namespace) — preventing a production field or variable literally named `test` from being marked as test, while still catching `py:tests` from `tests/__init__.py` where the module's leaf segment is the directory name itself.
2. **Test-prefix segment**: any segment starts with the literal prefix `test_` (catches `test_detect` modules and `test_handles_redirect` functions).
3. **Test-suffix segment**: any segment ends with the literal suffix `_test` (catches the `foo_test.py` module convention from pytest's `python_files = ["*_test.py"]` discovery). When the matching segment is **non-leaf** (e.g., methods inside a `foo_test.py` module — public_id `py:foo_test.some_method`), the rule fires unconditionally. When the matching segment is the **leaf**, the rule fires only when `kind.is_scope()` — catches the module init for `foo_test.py` itself (public_id `py:foo_test`, kind = Module) while excluding variables and fields literally ending in `_test` (e.g., `previous_test`, `expected_test`). Symmetric to rule 1's leaf scope-kind branch.
4. **Pytest conftest leaf**: the LEAF segment is exactly `conftest`.
5. **Unittest class shape**: the LEAF segment matches a unittest class shape AND `kind.is_class_like()` (`Class` / `Struct` / `Trait` / `Interface` / `Enum` / `TypeAlias`): either starts with `Test` (e.g., `TestParser`) or ends with `Test` / `TestCase` (e.g., `ParserTest`, `ParserTestCase`). The class-shape constraint prevents marking a production field or function literally named `test`.

The function MUST short-circuit on the first matching rule; ordering of evaluation MAY be implementation-defined.

This requirement provides Python's baseline test detection. It runs AFTER the existing file-glob path (`workspace.is_test_path(&relative_path)`) in the transform's `is_test_file || is_test_descriptor(...)` short-circuit, so users who configure `[tests].paths` retain full override authority. Users who DON'T configure `[tests].paths` (today's `TestsConfig::default()` returns an empty list) get conventional Python test marking automatically.

#### Scenario: Module under tests/ directory marked as test

- **WHEN** scip-python emits a function whose public_id is `py:tests.test_detect.test_handles_redirect`
- **THEN** the resulting `SymbolRecord.test` MUST be `true`
- **AND** at least one of rule 1 (non-leaf `tests`) or rule 2 (`test_*` prefix on `test_detect` / `test_handles_redirect`) MUST match

#### Scenario: tests/__init__.py module marked as test (leaf scope-kind fallback)

- **WHEN** scip-python emits the module init for `tests/__init__.py` whose public_id is `py:tests` (single segment, `kind = Module`)
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 1's leaf scope-kind branch
- **AND** the rule MUST NOT fire on the equivalent shape with non-scope kind (see "Production field named `test` NOT marked")

#### Scenario: Module named test_detect at top level

- **WHEN** scip-python emits a class whose public_id is `py:test_detect.TestDetect` AND `[tests].paths` is empty
- **THEN** the resulting `SymbolRecord.test` MUST be `true` (rule 2 on the `test_detect` segment; rule 5 also fires on the `TestDetect` leaf class shape)

#### Scenario: Fixture function inside tests/conftest.py

- **WHEN** scip-python emits a fixture function whose public_id is `py:tests.conftest.client_fixture` AND `kind = Function`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 1's non-leaf branch on the `tests` segment
- **AND** rule 4 MUST NOT fire here (rule 4 requires the leaf to be exactly `conftest`; the leaf is `client_fixture`)

#### Scenario: conftest.py module init at top level (rule 4 leaf match)

- **WHEN** scip-python emits the module init for `conftest.py` at the workspace root whose public_id is `py:conftest` AND `kind = Module`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 4 (leaf is exactly `conftest`)
- **AND** rule 4 is the sole reason — rule 1 does not match (`conftest` is not in {`tests`,`test`,`__tests__`}); rule 2/3 prefix/suffix don't fire; rule 5 requires class-like kind

#### Scenario: unittest TestCase subclass in non-test file

- **WHEN** scip-python emits a class `py:graphify.smoke.SmokeTestCase` AND `kind` is class-like
- **THEN** the resulting `SymbolRecord.test` MUST be `true` (rule 5: leaf ends with `TestCase`, class kind)

#### Scenario: Test class with `Test` prefix in non-test file (rule 5 starts-with branch in isolation)

- **WHEN** scip-python emits a class `py:graphify.TestParser` AND `kind = Class`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 5's "leaf starts with `Test`" branch
- **AND** rule 5 is the sole reason — rule 1 doesn't match (`TestParser` is not in {`tests`,`test`,`__tests__`}); rule 2's `test_` prefix doesn't match (`TestParser` starts with capital `T`, no underscore); rule 3 doesn't fire (no `_test` suffix); rule 4 requires literal leaf `conftest`

#### Scenario: foo_test.py module init marked as test (rule 3 leaf scope-kind branch)

- **WHEN** scip-python emits the module init for `foo_test.py` whose public_id is `py:foo_test` AND `kind = Module`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 3's leaf scope-kind branch
- **AND** rule 3 is the sole reason — rule 1 doesn't match (`foo_test` is not in {`tests`,`test`,`__tests__`}); rule 2's `test_` prefix doesn't fire (`foo_test` doesn't start with `test_`); rule 4 requires literal leaf `conftest`; rule 5 requires the leaf to start with `Test` or end with `Test`/`TestCase`

#### Scenario: Method inside foo_test.py marked as test (rule 3 non-leaf)

- **WHEN** scip-python emits a method `py:foo_test.helper_function` AND `kind = Function`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 3's non-leaf branch on `foo_test`
- **AND** the rule fires regardless of `kind` here because `foo_test` is non-leaf (leaf is `helper_function`)

#### Scenario: Production field named `test` NOT marked

- **WHEN** scip-python emits a field whose public_id is `py:graphify.config.test` (a config flag) AND `kind = Field`
- **THEN** the resulting `SymbolRecord.test` MUST be `false`
- **AND** the reasoning is: rule 1's leaf-segment branch requires `kind.is_scope()` and `Field` is not scope; rule 2/3 prefix/suffix don't fire on bare `test`; rule 4 requires the literal leaf `conftest`; rule 5 requires class-like kind

#### Scenario: Variable ending in `_test` NOT marked

- **WHEN** scip-python emits a module-level variable whose public_id is `py:graphify.runner.previous_test` AND `kind = Variable`
- **THEN** the resulting `SymbolRecord.test` MUST be `false`
- **AND** the reasoning is: rule 3 (`_test` suffix) is restricted to non-leaf segments; `previous_test` is the leaf

#### Scenario: User's [tests].paths override retains precedence

- **WHEN** the user configures `[tests].paths = ["foo/**"]` and scip-python emits a `Document` with `relative_path = "foo/bar.py"`
- **THEN** every symbol in `foo/bar.py` MUST be marked test via the file-glob, regardless of whether any descriptor rule fires
- **AND** the descriptor heuristic MUST NOT need to be consulted (the file-level glob short-circuits per the existing `is_test_file || is_test_descriptor(...)` evaluation order)

### Requirement: Workspace exclude-glob fallback at config load

The indexer SHALL replace the cleanup-era global `[exclude].globs` fallback with per-language excludes scoped to each language's pipeline. The `[exclude]` section MUST be removed from the config schema.

The exclude model SHALL be:

- `[workspace].excludes: Vec<String>` (optional, default `[]`) — workspace-wide patterns. The runtime SHALL hardcode `.git/**` and `**/.git/**` plus the auto-discovered linked git worktrees from `Workspace::excluded_dirs()`. `[workspace].excludes` SHALL be the ONLY exclude set consulted cross-language; `Workspace::canonicalize` consults it and gates documents from every driver.
- `[language.X].excludes: Vec<String>` for every supported language (`rust`, `typescript`, `csharp`, `python`). Each field SHALL default to that language's conventional set via `*Config::DEFAULT_EXCLUDES` — the serde default produces a fresh `Vec<String>` from the constant. User-supplied values MUST REPLACE the default fully; no implicit merge.

Per-language `excludes` SHALL be consulted EXCLUSIVELY by that language's driver (discovery walker) and that language's transform (per-document filter). They MUST NOT gate documents emitted by other languages.

The previous global `[exclude].globs` and Python's `[language.python].exclude_documents` MUST be removed. TOML files containing either field MUST cause config load to fail under `deny_unknown_fields`.

#### Scenario: Rust-only workspace, no `[exclude]` section

- **WHEN** only `[language.rust].enabled = true` and no `[exclude]` or `[workspace].excludes`
- **THEN** the resolved Rust exclude set MUST be `RustConfig::DEFAULT_EXCLUDES` (`target/**`, `**/target/**`)
- **AND** the workspace exclude set MUST be exactly `.git/**`, `**/.git/**` plus auto-discovered worktrees
- **AND** Python's, TypeScript's, C#'s excludes MUST NOT influence canonicalize or any driver

#### Scenario: Python-only workspace, no `[exclude]` section

- **WHEN** only `[language.python].enabled = true` and no `[exclude]` or `[workspace].excludes`
- **THEN** the resolved Python exclude set MUST be `PythonConfig::DEFAULT_EXCLUDES`
- **AND** the workspace exclude set MUST contain ONLY `.git/**` and `**/.git/**` (plus worktrees)
- **AND** a `target/foo.py` Document from scip-python MUST be ingested normally (Rust's `target/**` does not influence Python's pipeline)

#### Scenario: User-supplied per-language excludes replace defaults

- **WHEN** the user configures `[language.python] excludes = ["worked/**"]`
- **THEN** the resolved Python exclude set MUST be exactly `["worked/**"]`
- **AND** `__pycache__/foo.py` MUST be ingested (the default was replaced; user did not list `__pycache__/**`)

#### Scenario: Explicit empty list opts out

- **WHEN** the user configures `[language.python] excludes = []`
- **THEN** the resolved Python exclude set MUST be empty
- **AND** the Python driver and transform MUST NOT skip any path on Python's behalf

#### Scenario: Workspace excludes gate cross-language

- **WHEN** the user configures `[workspace] excludes = ["sensitive/**"]` AND BOTH Python and C# are enabled
- **THEN** a Document with `relative_path = "sensitive/foo.py"` MUST be rejected by canonicalize for Python
- **AND** a Document with `relative_path = "sensitive/foo.cs"` MUST be rejected by canonicalize for C#

#### Scenario: Per-language exclude does NOT leak across languages

- **WHEN** `[language.python] excludes = ["__pycache__/**"]` (the default) AND `[language.csharp].enabled = true`
- **AND** an out-of-band SCIP file from kenn-dotnet contains a Document with `relative_path = "__pycache__/foo.cs"`
- **THEN** the C# transform MUST ingest that Document normally
- **AND** the path MUST NOT be rejected by canonicalize

#### Scenario: Legacy `[exclude]` section is a hard error

- **WHEN** the user's TOML contains `[exclude] globs = ["foo/**"]`
- **THEN** config load MUST fail with a `deny_unknown_fields` error naming the `[exclude]` section
- **AND** no fallback substitution MUST occur

#### Scenario: Legacy `exclude_documents` field is a hard error

- **WHEN** the user's TOML contains `[language.python] exclude_documents = ["worked/**"]`
- **THEN** config load MUST fail with a `deny_unknown_fields` error naming the field

### Requirement: Per-language `is_excluded` API on `Workspace`

`Workspace` SHALL expose `is_excluded(language: Language, relative_path: &str) -> bool`. The function:

1. Normalizes the relative path to forward-slash separators (mirroring `canonicalize`'s normalization).
2. Matches the normalized path against that language's `GlobSet` only.
3. Returns `true` on match, `false` otherwise.

The workspace-level exclude check (cross-language, performed by `canonicalize`) is NOT consulted by `is_excluded` — canonicalize is the only path through which workspace excludes apply, and `is_excluded` is for callers downstream of canonicalize.

#### Scenario: Per-language match on macOS / Linux

- **WHEN** Python's exclude set contains `__pycache__/**` AND the relative path is `__pycache__/foo.py`
- **THEN** `workspace.is_excluded(Language::Python, "__pycache__/foo.py")` MUST return `true`

#### Scenario: Windows-style separator does not break match

- **WHEN** Python's exclude set contains `__pycache__/**` AND the relative path representation uses `\\` (constructed via `Path::new("__pycache__\\\\foo.py")` on Windows)
- **THEN** the normalized form MUST match the pattern; `is_excluded` MUST return `true`

#### Scenario: Other languages are not consulted

- **WHEN** Python's exclude set contains `__pycache__/**` AND C#'s exclude set is empty
- **THEN** `workspace.is_excluded(Language::Csharp, "__pycache__/foo.cs")` MUST return `false`

### Requirement: Workspace-aware discovery walker prunes excluded directories per language

SCIP drivers' discovery walkers SHALL use a workspace-aware helper `walk_for_language(workspace, language)` that prunes directory recursion when EITHER the directory matches `workspace.workspace_excludes` OR `workspace.is_excluded(language, dir)`. A populated `.venv/` (for Python) or `target/` (for Rust) MUST NOT be descended into; the walker MUST NOT call `read_dir` on such directories.

This requirement subsumes the pruning behavior introduced by `kenn-default-excludes-cleanup` and corrects the per-file post-filter perf regression noted in the review of that change.

#### Scenario: Walker prunes a language-specific excluded directory

- **WHEN** `walk_for_language(workspace, Language::Python)` is called on a workspace where Python's excludes contain `.venv/**` AND `.venv/lib/site-packages/foo.py` exists
- **THEN** the iterator MUST NOT yield `.venv/lib/site-packages/foo.py`
- **AND** the implementation MUST NOT call `read_dir` on `.venv/`

#### Scenario: Walker for a different language does not prune

- **WHEN** `walk_for_language(workspace, Language::Csharp)` is called on the same workspace with Python excludes `.venv/**` AND C# excludes empty
- **THEN** the iterator MUST yield files under `.venv/` (assuming none match C#'s extension filter — the prune does not fire because Python's excludes don't influence the C# walker)

### Requirement: Per-language transform consults its own exclude set at ingest

The SCIP→record transform for language `L` SHALL consult `workspace.is_excluded(L, &document.relative_path)` before emitting any record from a Document. When the check returns `true`, the transform MUST drop the document: no `SymbolRecord`, no `DefRecord`, no occurrence-derived edge. Cross-document edges referencing symbols defined inside a dropped document continue to emit via the existing `external_symbols` path (`is_external = true` stubs).

The Python-only `is_python_excluded_document` introduced by `kenn-python-scoping` is removed. Its responsibilities are subsumed by `is_excluded(Language::Python, ...)`.

#### Scenario: Python transform drops document matching its excludes

- **WHEN** Python's excludes contain `worked/**` AND scip-python emits a Document with `relative_path = "worked/httpx/raw/transport.py"`
- **THEN** the transform MUST NOT emit any record from that Document

#### Scenario: Non-matching Python document is ingested normally

- **WHEN** Python's excludes contain `worked/**` AND scip-python emits a Document with `relative_path = "graphify/detect.py"`
- **THEN** the transform MUST ingest every record per the existing requirements

#### Scenario: C# transform unaffected by Python's excludes

- **WHEN** Python's excludes contain `__pycache__/**` AND kenn-dotnet emits a Document with `relative_path = "__pycache__/foo.cs"`
- **THEN** the C# transform MUST ingest the Document normally (Python's exclude set is not consulted by the C# transform)

### Requirement: SCIP definition enclosing_range populates the def body extent

The SCIP transform SHALL read `Occurrence.enclosing_range` for each
**definition** occurrence and, when present, map it onto the `DefRecord` body
extent (`body_start_line` / `body_end_line`, 1-based). Like `range`,
`enclosing_range` is 0-based on both axes and comes in the 3-int (single-line)
or 4-int (multi-line) shape; the transform SHALL add `+1` to the line values,
matching the name-range convention.

When `enclosing_range` is empty — an older rust-analyzer that does not emit it,
or a producer (scip-go / scip-python) that omits it for a given occurrence — the
body extent SHALL be `0` (absent). The zero-range synthetic placeholder emitted
for a symbol with no definition occurrence SHALL also carry a `0` body extent.

Capturing the body extent MUST NOT change the def's name range, nor the
`DocumentDefIndex` used for edge FROM-attribution (which already reads
`enclosing_range` independently).

#### Scenario: a definition with enclosing_range gets a body span

- **WHEN** a definition occurrence reports `range = [45, 0, 45, 11]` and
  `enclosing_range = [41, 0, 236, 1]` (0-based)
- **THEN** the resulting `DefRecord` MUST have `start_line = 46` (name, `+1`)
- **AND** `body_start_line = 42`, `body_end_line = 237` (enclosing, `+1`)

#### Scenario: a definition without enclosing_range gets a zero body span

- **WHEN** a definition occurrence reports a `range` but an empty
  `enclosing_range` (e.g. a pre-Dec-2025 rust-analyzer)
- **THEN** the resulting `DefRecord` MUST have `body_start_line = 0` and
  `body_end_line = 0`
- **AND** `get_source` for that symbol falls back to the declaration line

### Requirement: a too-old rust-analyzer is surfaced, not silently degraded

The indexer SHALL emit a one-time warning when a completed Rust index yields
**zero** definition body extents, identifying the resolved rust-analyzer as too
old for full-item `get_source` and recommending an upgrade (Homebrew
`rust-analyzer` or `rustup update`). `rust-analyzer` emits
`Occurrence.enclosing_range` on definitions only from ~Dec-2025 onward, and the
rustup-bundled build lags the standalone release. Indexing SHALL otherwise
succeed; Rust `get_source` degrades to declaration lines.

#### Scenario: old rust-analyzer triggers an upgrade warning

- **WHEN** a Rust index completes and no definition carried an `enclosing_range`
- **THEN** the run SHALL log a warning naming the too-old rust-analyzer and the
  upgrade path
- **AND** the index SHALL still publish successfully

