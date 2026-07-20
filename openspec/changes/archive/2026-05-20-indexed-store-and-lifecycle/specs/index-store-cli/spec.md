## ADDED Requirements

### Requirement: Single binary, subcommand dispatch

The project SHALL ship a single binary `kenn` whose first positional argument is the subcommand. Top-level subcommands defined by this proposal: `init`, `index`, `status`, `rollback`. The binary SHALL accept `--workspace <path>` (default: current working directory's git toplevel, falling back to current working directory) and `--config <path>` (default: `<workspace>/kenn.toml`).

#### Scenario: Invocation outside any workspace

- **WHEN** `kenn <subcommand>` is invoked outside any directory that resolves to a workspace
- **THEN** the command MUST exit with a non-zero status and a clear error indicating no workspace was identified

### Requirement: `kenn init`

`init` SHALL create the `.kenn/` directory at the workspace root if absent, write a starter `kenn.toml` with sane defaults if absent, and emit a one-line confirmation. It MUST be idempotent: running on an already-initialized workspace MUST exit successfully and report "already initialized" without modifying any existing files.

#### Scenario: Fresh workspace

- **WHEN** `kenn init` is run in a workspace that has no `.kenn/`
- **THEN** `.kenn/` MUST be created
- **AND** a `kenn.toml` MUST be created with the language defaults (e.g., C# enabled if a `.sln` is found)
- **AND** the command MUST exit with status 0

#### Scenario: Already-initialized workspace

- **WHEN** `kenn init` is run where `.kenn/` already exists
- **THEN** no files MUST be modified
- **AND** the command MUST exit with status 0 and report "already initialized"

### Requirement: `kenn index`

`index` SHALL run the indexer pipeline (per `scip-indexing-pipeline` behavior) and, on successful completion, perform the lifecycle flip. By default it MUST consult the staleness signal and skip if the workspace is unchanged. The flag `--force` MUST bypass the skip. The command MUST exit with status 0 on success or skip; non-zero on failure (lock contention, indexer Failed, no producer available).

#### Scenario: Skip due to git-aware staleness match

- **WHEN** `kenn index` runs and the staleness key matches the current snapshot's recorded key
- **THEN** the command MUST exit with status 0
- **AND** stdout MUST clearly state that the run was skipped and why

#### Scenario: Run progresses with periodic progress

- **WHEN** `kenn index` runs an actual indexer
- **THEN** the command MUST emit human-readable progress updates (at least: phase transitions — discovery, per-unit start/finish, parsing, ingest)
- **AND** with `--json` flag, MUST emit one JSON line per progress event

### Requirement: `kenn status`

`status` SHALL print the current state of the local `.kenn/`: whether `live` exists, when the current snapshot was created, summary metrics from its run report (doc/symbol/definition/edge counts), failed projects from the most recent run, and any pending warnings (e.g., last flip's regression warnings). When no local index exists but a parent fallback is available, `status` MUST identify the fallback path and warn that the data is read from the parent.

#### Scenario: Healthy local snapshot

- **WHEN** `live → snapshots/T0/` and the run reported Success
- **THEN** `kenn status` MUST print the snapshot timestamp, key counts, and `status: ok`

#### Scenario: Worktree using parent fallback

- **WHEN** the workspace has no local `.kenn/live` but the parent does
- **THEN** `kenn status` MUST identify the parent path and label the state `fallback: parent`

#### Scenario: Last flip had regressions

- **WHEN** the most recent flip emitted a doc-count regression warning
- **THEN** `kenn status` MUST surface that warning in its output

### Requirement: `kenn rollback`

`rollback` SHALL atomically flip `live` to the previous retained snapshot, per the lifecycle requirements. It MUST require explicit confirmation in interactive use (e.g., a `--yes` flag is required when stdout is not a TTY, or a confirmation prompt when it is). On success, it MUST emit the new `live` target.

#### Scenario: Confirmed rollback in TTY

- **WHEN** the user runs `kenn rollback` in a TTY
- **AND** confirms the prompt
- **THEN** `live` MUST flip to the previous snapshot
- **AND** the command MUST print the new `live` target

#### Scenario: Non-TTY without `--yes`

- **WHEN** `kenn rollback` runs in a non-TTY context without `--yes`
- **THEN** the command MUST exit with non-zero status without modifying anything

### Requirement: Exit codes

Each subcommand SHALL use stable exit codes documented in `--help`. At minimum:
- `0` — success or skip
- `1` — generic error
- `2` — usage error (bad flag, missing argument)
- `3` — workspace not identified
- `4` — lock contention (another writer is active)
- `5` — indexer reported `Failed`

#### Scenario: Lock contention exit code

- **WHEN** an `index` run is already in progress and a second `kenn index` is invoked
- **THEN** the second invocation MUST exit with status code `4`
