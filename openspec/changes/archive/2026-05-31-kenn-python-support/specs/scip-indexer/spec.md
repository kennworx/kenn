# scip-indexer

## MODIFIED Requirements

### Requirement: Indexable-unit discovery

The indexer SHALL discover indexable units by scanning the workspace for files matching configured patterns per language (e.g., `**/*.sln` for C#, `Cargo.toml` for Rust, `*.go`/module roots for Go, **workspace root for Python**). TypeScript is no longer discovered by the SCIP path — it is produced by the `typescript-stream-indexer` (`kenn-ts`) JSONL producer. The discovery rule for each language MUST be configurable. Files under configured **explicit-exclude globs** (default: `node_modules/`, `bin/`, `obj/`, `target/`) MUST be skipped, and additionally files under any **other-worktree directories** (see *Git-aware worktree exclusion*) MUST be skipped. For Python the indexer SHALL additionally skip `__pycache__/`, `.venv/`, `venv/`, and `.kenn/` (`node_modules/` is already covered by the default explicit-exclude globs).

The Python rule replaces the earlier "package roots for Python" phrasing: scip-python loads the project graph regardless of scoping, so emitting one unit at the workspace root is both correct and the minimum-work scheduling. When at least one `.py` (or `.pyi`) file is present under the workspace root (after all exclusions), the SCIP indexer MUST emit exactly one Python unit whose path is the workspace root. When no `.py`/`.pyi` file is found after exclusions, the SCIP indexer MUST emit zero Python units and MUST NOT spawn `scip-python`.

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

## ADDED Requirements

### Requirement: Python indexer dispatch via launcher command

When a Python unit has been discovered, the indexer driver SHALL invoke `scip-python` by spawning the configured launcher command (a non-empty sequence of tokens), passing `index --cwd <workspace-root> --output <derived-store>/index-python.scip --quiet` as the trailing arguments. When the configured `project_name` or `project_version` is set, the driver MUST forward each as `--project-name <name>` and `--project-version <version>` respectively. The produced `.scip` SHALL be ingested through the existing SCIP output-parsing requirement and the `PythonTransformer` rewrites `scip-python python <dist> <ver> <descriptor>` symbols into `py:<module>.<...>` public IDs.

#### Scenario: Default launcher invokes scip-python directly

- **WHEN** the user configures `[language.python] enabled = true` with the default `command = ["scip-python"]`
- **THEN** the driver MUST spawn `scip-python index --cwd <ws> --output <derived>/index-python.scip --quiet`

#### Scenario: Launcher routes through bunx

- **WHEN** the user configures `command = ["bunx", "@sourcegraph/scip-python"]`
- **THEN** the driver MUST spawn `bunx @sourcegraph/scip-python index --cwd <ws> --output <derived>/index-python.scip --quiet`

#### Scenario: Launcher routes through npx

- **WHEN** the user configures `command = ["npx", "--yes", "@sourcegraph/scip-python"]`
- **THEN** the driver MUST spawn `npx --yes @sourcegraph/scip-python index --cwd <ws> --output <derived>/index-python.scip --quiet`

#### Scenario: project_name and project_version forwarded when set

- **WHEN** the user configures `project_name = "graphify"` and `project_version = "0.8.12"`
- **THEN** the driver MUST include `--project-name graphify --project-version 0.8.12` in the spawn arguments

#### Scenario: Python symbols become py: public IDs

- **WHEN** `scip-python` emits a symbol `scip-python python graphify 0.8.12 graphify/detect/detect_languages().`
- **THEN** the resulting record's public ID MUST be `py:graphify.detect.detect_languages`

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

