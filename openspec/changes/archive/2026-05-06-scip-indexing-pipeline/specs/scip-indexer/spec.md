## ADDED Requirements

### Requirement: Out-of-band execution

The SCIP indexer SHALL run as an out-of-band process — invoked by an explicit command, file-watcher event, or scheduler — and MUST NOT be invoked during MCP request handling. Downstream consumers (DB ingest, MCP queries) read from previously persisted data; they never block on a live indexer run.

#### Scenario: MCP server starts mid-index

- **WHEN** an MCP session starts while a SCIP indexer run is in progress
- **THEN** the MCP server MUST serve queries from the previously persisted data without waiting for the indexer
- **AND** newly produced data from the in-progress run MUST become visible only after the run completes and is committed

### Requirement: Indexable-unit discovery

The indexer SHALL discover indexable units by scanning the workspace for files matching configured patterns per language (e.g., `**/*.sln` for C#, `package.json` with a `tsconfig.json` for TypeScript, `Cargo.toml` for Rust). The discovery rule for each language MUST be configurable. Files under configured **explicit-exclude globs** (default: `node_modules/`, `bin/`, `obj/`, `target/`) MUST be skipped, and additionally files under any **other-worktree directories** (see *Git-aware worktree exclusion*) MUST be skipped.

#### Scenario: Multiple solutions in one workspace

- **WHEN** a workspace contains both `App.sln` and `Worker/Worker.sln`
- **THEN** the indexer MUST treat each as a distinct indexable unit
- **AND** SHALL run scip-dotnet once per unit

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

The indexer driver SHALL maintain a registry mapping (language, indexable-unit-kind) to a SCIP indexer command. For C#: `scip-dotnet index <sln>`. For TypeScript: `scip-typescript index`. For Python: `scip-python index`. For Go: `scip-go index`. For Rust: `rust-analyzer scip`. The registry MUST be extensible without code changes when reasonable (config-driven).

#### Scenario: Adding a new language indexer

- **WHEN** a new entry is registered mapping `(language="kotlin", unit=".gradle.kts")` to a `scip-kotlin` command
- **THEN** the indexer driver MUST pick up Kotlin units in subsequent runs without code changes

### Requirement: Tier-2 availability detection

For each language enabled in configuration, the indexer driver SHALL probe whether the corresponding SCIP binary is available and runnable. If unavailable, the driver MUST emit a structured "Tier 2 unavailable" status for that language and continue with other languages. The system MUST NOT abort.

#### Scenario: scip-dotnet not installed

- **WHEN** the SCIP driver runs in a workspace with C# projects but `scip-dotnet` is not on PATH and no fallback path is configured
- **THEN** the run MUST complete with a clear "Tier 2 unavailable for csharp: scip-dotnet not found" status
- **AND** any other-language indexers configured MUST still run

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

Each indexer run SHALL produce a structured run report containing: indexer name and version, indexable unit identifier, start/end timestamps, total documents/symbols/occurrences/edges produced, list of failed projects with diagnostics, and a status field of `success | partial | failed`. The report MUST be persisted alongside the produced data-model records.

#### Scenario: Querying the latest run for a unit

- **WHEN** a consumer asks for the latest run report for `App.sln`
- **THEN** the system MUST return the most recent report including its status, counts, and any failures

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
