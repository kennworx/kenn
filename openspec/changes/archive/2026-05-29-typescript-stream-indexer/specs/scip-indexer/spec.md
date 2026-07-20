## MODIFIED Requirements

### Requirement: Indexable-unit discovery

The indexer SHALL discover indexable units by scanning the workspace for files matching configured patterns per language (e.g., `**/*.sln` for C#, `Cargo.toml` for Rust, `*.go`/module roots for Go, package roots for Python). TypeScript is no longer discovered by the SCIP path — it is produced by the `typescript-stream-indexer` (`kenn-ts`) JSONL producer. The discovery rule for each language MUST be configurable. Files under configured **explicit-exclude globs** (default: `node_modules/`, `bin/`, `obj/`, `target/`) MUST be skipped, and additionally files under any **other-worktree directories** (see *Git-aware worktree exclusion*) MUST be skipped.

#### Scenario: Multiple solutions in one workspace

- **WHEN** a workspace contains both `App.sln` and `Worker/Worker.sln`
- **THEN** the indexer MUST treat each as a distinct indexable unit
- **AND** SHALL run scip-dotnet once per unit

#### Scenario: TypeScript is not a SCIP unit

- **WHEN** a workspace contains `tsconfig.json` projects
- **THEN** the SCIP indexer MUST NOT discover them as SCIP units (they are handled by the `kenn-ts` JSONL producer)

### Requirement: Per-language indexer dispatch

The indexer driver SHALL maintain a registry mapping (language, indexable-unit-kind) to a SCIP indexer command. For C#: `scip-dotnet index <sln>`. For Python: `scip-python index`. For Go: `scip-go index`. For Rust: `rust-analyzer scip`. TypeScript SHALL NOT have a SCIP registry entry — it is produced by the `kenn-ts` JSONL producer, not a `scip-*` binary. The registry MUST be extensible without code changes when reasonable (config-driven).

#### Scenario: Adding a new language indexer

- **WHEN** a new entry is registered mapping `(language="kotlin", unit=".gradle.kts")` to a `scip-kotlin` command
- **THEN** the indexer driver MUST pick up Kotlin units in subsequent runs without code changes

#### Scenario: TypeScript has no SCIP command

- **WHEN** the SCIP driver registry is consulted for `language="typescript"`
- **THEN** there is no entry (TypeScript indexing is the `kenn-ts` JSONL producer's responsibility)
