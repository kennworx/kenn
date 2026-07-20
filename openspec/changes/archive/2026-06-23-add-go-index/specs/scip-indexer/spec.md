# scip-indexer

## MODIFIED Requirements

### Requirement: Indexable-unit discovery

The indexer SHALL discover indexable units by scanning the workspace for files matching configured patterns per language (e.g., `**/*.sln` for C#, `Cargo.toml` for Rust, **one unit per `go.mod` module root for Go**, **workspace root for Python**). TypeScript is no longer discovered by the SCIP path — it is produced by the `typescript-stream-indexer` (`kenn-ts`) JSONL producer. The discovery rule for each language MUST be configurable. Files under configured **explicit-exclude globs** (default: `node_modules/`, `bin/`, `obj/`, `target/`) MUST be skipped, and additionally files under any **other-worktree directories** (see *Git-aware worktree exclusion*) MUST be skipped. For Python the indexer SHALL additionally skip `__pycache__/`, `.venv/`, `venv/`, and `.kenn/`. For Go the indexer SHALL additionally skip `vendor/` and `testdata/` (so a `go.mod` under vendored dependencies or test fixtures does not become its own unit).

The Go rule makes the earlier "`*.go`/module roots for Go" phrasing concrete: scip-go is module-scoped (`--module-root` points at a single `go.mod`), so the indexer emits **one unit per discovered `go.mod`**, each unit's path being the directory containing that `go.mod`. When at least one `go.mod` is present under the workspace root (after all exclusions), the SCIP indexer MUST emit one Go unit per `go.mod`. When no `go.mod` is found after exclusions, the SCIP indexer MUST emit zero Go units and MUST NOT spawn `scip-go`.

#### Scenario: Multiple solutions in one workspace

- **WHEN** a workspace contains both `App.sln` and `Worker/Worker.sln`
- **THEN** the indexer MUST treat each as a distinct indexable unit
- **AND** SHALL run scip-dotnet once per unit

#### Scenario: TypeScript is not a SCIP unit

- **WHEN** a workspace contains `tsconfig.json` projects
- **THEN** the SCIP indexer MUST NOT discover them as SCIP units (they are handled by the `kenn-ts` JSONL producer)

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

## ADDED Requirements

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

- **WHEN** `scip-go` emits a symbol `scip-go gomod github.com/foo/quinn-proto 0.1.0 connection/Connection#New().`
- **THEN** the resulting record's public ID MUST be `go:github.com/foo/quinn-proto/connection.Connection.New`
