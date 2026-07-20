## Why

We are building an MCP server that gives AI agents code structure (symbol search, find-references, navigation, call hierarchy, dependency graphs) over multi-language monorepos (C#, TypeScript/React, Rust, Python, Go), starting with C#. The first concrete capability we need is a way to **get accurate code structure data out of source code and into a normalized representation** that downstream components (a queryable index, an MCP tool layer) can consume.

Our spike validated that running existing **SCIP indexers** out-of-band is the cheapest path to high-quality intelligence: scip-dotnet on a 303k-LoC C# monorepo produced 122k definitions and 839k occurrences in 91 seconds, with inheritance edges and cross-package version info baked into the output. SCIP indexers exist for every language we plan to support, so this same pipeline scales beyond C# without rewriting the producer.

This proposal scopes only the **producer side**: orchestrating SCIP indexers and transforming their output into our internal data model. The data model is the contract that the eventual DB-ingest proposal and MCP-query proposal will both consume.

## What Changes

- Define a normalized **code structure data model** (symbols, occurrences, relationships, file-level and project-level dependency edges) derived from the SCIP protobuf shape but adapted for multi-source merging and project disambiguation
- Add a **SCIP indexer driver** that discovers indexable units (`.sln` files for C#, etc.), runs the appropriate `scip-*` binary out-of-band, and emits the normalized data model
- **Path canonicalization**: convert each indexer's `metadata.project_root + relative_path` to absolute then to workspace-relative, so multiple indexes covering overlapping code merge cleanly
- **Multi-source merge**: dedup on `(canonical_path, symbol_string, range)` rather than `symbol_string` alone, since SCIP's local-package descriptor (`nuget . .`) does not disambiguate projects with shared root namespaces
- **Worktree exclusion**: indexers must skip linked git worktrees (discovered via `git worktree list`, not by hard-coded path patterns — users put worktrees in many places) along with explicitly configured exclude globs
- **Failure tolerance**: per-project indexer failures (NuGet vulnerability errors, missing `.csproj` references, SDK version mismatches) must not abort the run; partial coverage is acceptable and reported
- Provide a `Directory.Build.props` recipe (or equivalent) so scip-dotnet does not block on MSBuildWorkspace vulnerability errors
- Indexing runs **out-of-band only** — never triggered by an MCP session; downstream consumers read from the persisted data, not from a live indexer

This proposal explicitly **defers**:
- Tree-sitter Tier-1 indexers (separate proposal — same data model)
- Embedded DB choice and bulk-ingest performance (separate proposal — consumes this data model)
- MCP tool surface (separate proposal — queries the DB)

## Capabilities

### New Capabilities

- `kenn-data-model`: the normalized representation of source-code structure (symbols, occurrences, relationships, file→file edges, project→project edges) that all indexers produce and all consumers read. Defines identity rules (canonical path + symbol + range), edge kinds (extends/implements/calls/field_type/param_type/return_type/instantiates/imports/contains), and how external-package references are represented. Indexer-agnostic.
- `scip-indexer`: the SCIP-specific producer. Discovers indexable units in a workspace, invokes the right `scip-*` binary per language, parses the resulting `.scip` protobuf, transforms it into the `kenn-data-model`, and reports per-unit success/partial/failed status. Handles path canonicalization, worktree exclusion, and merge-time dedup. **Includes a language-specific positional refinement** that fills in `Occurrence.enclosing_range` when the underlying SCIP indexer leaves it empty (verified empty in scip-dotnet and rust-analyzer; native in scip-typescript / scip-python / scip-go). The refinement reads the source file as text — no AST parser, no grammar dependency — and runs a small line classifier plus a per-language ruleset (parameter-kind exclusion, attribute-line re-anchor, same-line forward-def, collection-literal disambiguation for C#). Measured at 99.79 % FROM-attribution agreement with a tree-sitter reference implementation on a 303k-LoC C# corpus, at ~1.8× the cost of the bare heuristic (vs ~10× for tree-sitter). Activation is per-indexer based on declared coverage. **Also includes a SCIP-descriptor symbol-kind classifier** that derives `SymbolInformation.kind` from the SCIP symbol-string suffix (`#`/`().`/`.`/`(name)`/`/`/`[T]`) when the indexer leaves `kind` unset (verified empty in scip-dotnet, scip-typescript, scip-python; native in scip-go and rust-analyzer). Language-agnostic by SCIP spec. This explicitly does NOT introduce a Tier-1 AST-based indexer.

### Modified Capabilities

(None — this is the first proposal in the system.)

## Impact

- **New runtime dependency**: language-specific SCIP indexer binaries must be available for languages where Tier 2 is enabled. For C#: `scip-dotnet` requires .NET SDK matching the project's TFM. The indexer driver should report a clear "Tier 2 unavailable for X" rather than failing when a binary is absent.
- **Workspace prerequisite**: C# projects need a usable `Directory.Build.props` (or equivalent) so MSBuildWorkspace does not abort on common analyzer/audit errors. We should document this and offer a sample.
- **Disk**: per-workspace persisted intermediate representation. Empirical: ~220 bytes/LoC of source → ~220 MB for 1M LoC. Sized comfortably for an embedded DB later.
- **No public API yet**: this proposal produces an internal Rust crate / module with no external surface. The MCP tool surface lands in a later proposal.
- **No code in `src/` or production paths today**: the workspace is greenfield. This proposal lays the foundation crate.
