# workspace-init Specification

## Purpose
TBD - created by archiving change foreign-workspace-indexing. Update Purpose after archive.
## Requirements
### Requirement: Every command accepts a short workspace-targeting flag

The CLI SHALL accept `-w` as a short alias for the existing global
`--workspace <PATH>` flag. Resolution precedence is unchanged: an explicit
workspace path outranks `CLAUDE_PROJECT_DIR`, git top-level, and the current
working directory, and is tagged `WorkspaceSource::CliFlag`.

#### Scenario: A short flag targets a cloned repository

- **WHEN** a user runs `kenn index -w ./tmp/repo` from an unrelated directory
- **THEN** the workspace root is `./tmp/repo`
- **AND** the config is read from `./tmp/repo/kenn.toml`
- **AND** the store is created under `./tmp/repo/.kenn/`

#### Scenario: The short and long flags are equivalent

- **WHEN** a user runs `kenn status -w ./tmp/repo`
- **AND** a user runs `kenn status --workspace ./tmp/repo`
- **THEN** both resolve the same workspace root from the same source

### Requirement: `kenn init` detects languages by a pruned marker walk

`kenn init` SHALL discover which languages a workspace contains by walking it
once for marker files, and SHALL prune the walk using the union of every
language's `DEFAULT_EXCLUDES` constant. Because `init` runs before a config
exists, it SHALL take those excludes from the `kenn-config` constants directly
rather than from `kenn.toml`.

The walk SHALL be recursive, so a marker in a sub-package of a monorepo is found.
A marker inside a pruned directory SHALL NOT count as a detection.

Built-in producers that require no external command (markdown, CSS, HTML) SHALL
be enabled whenever their file types are present.

#### Scenario: A marker in a monorepo sub-package is detected

- **WHEN** `kenn init -w ./tmp/repo` runs against a workspace whose only `go.mod`
  is at `services/api/go.mod`
- **THEN** Go is detected

#### Scenario: A marker inside a pruned directory is ignored

- **WHEN** a workspace contains `vendor/example.com/dep/go.mod` and no other `go.mod`
- **THEN** Go is not detected
- **AND** the written config contains no `[language.go]` block

#### Scenario: Markdown is enabled without any external command

- **WHEN** `kenn init` runs against a workspace containing `.md` files
- **AND** no language indexer is installed
- **THEN** `[language.markdown] enabled = true` is written
- **AND** `kenn index` produces a non-empty snapshot

### Requirement: `kenn init` verifies an indexer by running it, not by finding it

`kenn init` SHALL determine an indexer's availability by executing its version
probe and requiring a successful exit. A spawn failure, a non-zero exit, or a
timeout SHALL be treated as unavailable. A file existing on `PATH` SHALL NOT by
itself be treated as availability, because a broken shim satisfies an existence
check and then fails at index time.

A language whose marker is found and whose probe succeeds SHALL be written as
`enabled = true`. `init` SHALL omit the `command` key in that case, since the
per-language default already resolves the tool on `PATH`.

#### Scenario: A working indexer enables its language

- **WHEN** `kenn init -w ./tmp/repo` runs against a workspace containing `Cargo.toml`
- **AND** `rust-analyzer --version` exits successfully
- **THEN** the written `kenn.toml` contains `[language.rust] enabled = true`
- **AND** the written `[language.rust]` block contains no `command` key
- **AND** a subsequent `kenn index -w ./tmp/repo` indexes Rust symbols

#### Scenario: A broken shim on PATH does not enable its language

- **WHEN** a `rust-analyzer` executable is present on `PATH`
- **AND** running its version probe exits non-zero
- **THEN** Rust is reported as degraded, not enabled
- **AND** `kenn index` is never left to fail on a config `init` wrote

### Requirement: A detected language with no working indexer degrades to the text fallback

`kenn init` SHALL leave a language disabled when its marker is found but its
version probe fails. It SHALL instead add that language's source-file globs to
`[language.text] include` and enable the text fallback, so the repository is
immediately searchable by full-text and semantic search with no symbol graph, no
external command, and no code from the workspace executed.

When degrading language X, `init` SHALL write `[language.text] excludes` as the
union of `TextConfig::DEFAULT_EXCLUDES` and `X::DEFAULT_EXCLUDES`. This is
required because a user-supplied `excludes` list replaces the defaults entirely,
and text's defaults do not cover vendored or build trees such as `vendor/**`,
`**/testdata/**`, `.venv/**`, or `obj/**`.

`init` SHALL report each degraded language, naming both the failing command and
the install hint that would upgrade it.

Where kenn publishes a Homebrew formula for the missing indexer, the hint SHALL
name that formula. A hint that describes a different installation route than
the one the user took to get kenn is a hint they cannot follow.

#### Scenario: A Go repository without a working scip-go

- **WHEN** `kenn init -w ./tmp/repo` runs against a workspace containing `go.mod`
- **AND** `scip-go` fails its version probe
- **THEN** `[language.go]` is not enabled
- **AND** `[language.text] enabled = true` is written with `**/*.go` among its `include` globs
- **AND** `[language.text] excludes` contains `vendor/**` and `**/testdata/**`
- **AND** the report names `scip-go` as missing and states the install command
- **AND** a subsequent `kenn index -w ./tmp/repo` makes `.go` files semantically searchable

#### Scenario: A C# repository without kenn-dotnet names the formula

- **WHEN** `kenn init` runs against a workspace containing `global.json`
- **AND** `kenn-dotnet` fails its version probe
- **THEN** the report names `kenn-dotnet` as missing
- **AND** the install hint names the Homebrew formula that provides it

#### Scenario: Vendored sources are not fallback-indexed

- **WHEN** Go degrades to the text fallback in a workspace with a populated `vendor/` tree
- **THEN** no file under `vendor/` is chunked, stored, or embedded

#### Scenario: Degrading one language does not degrade another

- **WHEN** a workspace contains both `Cargo.toml` and `go.mod`
- **AND** `rust-analyzer` probes successfully but `scip-go` does not
- **THEN** `[language.rust] enabled = true` is written
- **AND** Go source globs are added to the text fallback
- **AND** the report lists Rust as enabled and Go as degraded

#### Scenario: Upgrading a degraded language needs no config cleanup

- **WHEN** a workspace has `**/*.go` in `[language.text] include` from an earlier degrade
- **AND** `[language.go] enabled = true` is later set
- **THEN** the text producer skips `.go` files because an enabled producer claims the extension
- **AND** no `.go` file is indexed both as chunks and as symbols

### Requirement: `kenn init` seeds test-path globs when none are configured

`kenn init` SHALL populate `[tests] paths` with the test globs of every language
it enabled, and SHALL do so only when the existing `tests.paths` is empty (the
block absent, or present with an empty list). A non-empty list SHALL be left
untouched, on both the fresh and the `--force` paths.

This is required because `[tests] paths` is authoritative with no built-in
fallback, and it is load-bearing: it feeds `Workspace::with_test_globs` for the
SCIP producers and reaches the .NET driver as `--test-glob`. A config rendered
from detection alone would leave a workspace in which no file is ever recognized
as test code.

A **degraded** language SHALL contribute no test globs, because the text producer
records every chunk as non-test; its globs would have no effect. When `init`
enables a language whose globs it cannot add — because `tests.paths` is already
non-empty — it SHALL report the globs it would have added rather than modify the
list.

#### Scenario: A fresh workspace gets test globs for its enabled languages

- **WHEN** `kenn init -w ./tmp/repo` enables Rust and Go
- **AND** the workspace has no `kenn.toml`
- **THEN** the written `[tests] paths` contains Rust's and Go's test globs
- **AND** contains no globs for languages that were not enabled

#### Scenario: An existing test-path list is never modified

- **WHEN** a `kenn.toml` sets `[tests] paths = ["custom/**"]`
- **AND** `kenn init --force` enables Go
- **THEN** `[tests] paths` still contains exactly `["custom/**"]`
- **AND** the report names the Go test globs it would otherwise have added

#### Scenario: A degraded language contributes no test globs

- **WHEN** Go degrades to the text fallback
- **AND** Rust is enabled
- **THEN** the written `[tests] paths` contains Rust's test globs only
- **AND** no `.go` chunk is recorded as test code

### Requirement: `kenn init` reports its decisions and never prompts

`kenn init` SHALL be non-interactive. It SHALL print, for every language it
considered, whether the language was enabled, degraded to the text fallback, or
absent — and for each failing probe, a per-language install hint.

When a probe runs and the indexer emits a diagnostic, `init` SHALL report the
INDEXER'S OWN message in preference to the static per-language hint. The static
hint remains the fallback for an indexer that produced no diagnostic, including
every third-party indexer.

The indexer knows which dependency is missing; the static hint can only name
the tool. Discarding the specific message in favour of the generic one tells
the user to reinstall something already installed.

`init` SHALL distinguish an indexer that could not be executed from one that
executed and failed, because the fixes differ: install the indexer, versus the
indexer is present but something it needs is not.

The command SHALL NOT read from stdin, SHALL NOT depend on a TTY, and SHALL NOT
change behavior based on whether one is attached. Degradation SHALL NOT be an
error: the exit code is success whenever a config was written.

#### Scenario: Init is run non-interactively by an agent

- **WHEN** `kenn init -w ./tmp/repo` runs with stdin closed and no TTY attached
- **THEN** the command completes without blocking
- **AND** the report distinguishes enabled, degraded, and absent languages
- **AND** the exit code is success even when every language degraded

#### Scenario: A present indexer that cannot run reports its own reason

- **WHEN** an indexer is on `PATH` but fails its probe, writing an `error:`
  line to stderr naming a missing dependency and the command that installs it
- **THEN** the report shows that message
- **AND** it does not show the static hint in its place

#### Scenario: An absent indexer falls back to the static hint

- **WHEN** an indexer cannot be executed at all
- **THEN** the report shows the static per-language install hint
- **AND** the line distinguishes "not installed" from "installed but failing"

### Requirement: `kenn init --force` merges the language config and preserves the rest

Without `--force`, `kenn init` SHALL preserve today's behavior: an existing
`kenn.toml` is never overwritten, and the command SHALL name the flag that would
rewrite it.

With `--force` against a parseable config, `init` SHALL replace only the
`[language.*]` section and SHALL preserve every other section's values —
including `[tests] paths`, which is authoritative with no built-in fallback, so
dropping it would silently stop the workspace from recognizing any test code.
`init` SHALL write `kenn.toml.bak` before rewriting, because a typed round-trip
preserves values but not comments.

#### Scenario: Re-running init without --force preserves user edits

- **WHEN** `kenn init` runs against a workspace whose `kenn.toml` a user has edited
- **THEN** the file is left byte-for-byte unchanged
- **AND** the report states that `--force` would rewrite it

#### Scenario: --force preserves non-language configuration

- **WHEN** a `kenn.toml` sets `[tests] paths`, `[layout] derived_root`, and `[metrics]`
- **AND** `kenn init --force` runs
- **THEN** the rewritten file still carries those three sections with the same values
- **AND** `kenn.toml.bak` holds the original file verbatim

### Requirement: `kenn init` remains runnable against an unparseable configuration

`kenn init` SHALL NOT fail when `<workspace>/kenn.toml` exists but cannot be
parsed. It SHALL warn, resolve the store layout against `Config::default()`, and
with `--force` SHALL replace the file after backing it up. A merge is impossible
in this case, so `init` SHALL state that non-language settings were discarded.

This holds even though every other workspace-bound command loads the config
before dispatch and fails on a parse error.

#### Scenario: Init recovers a workspace whose kenn.toml is unparseable

- **WHEN** a cloned repository contains a `kenn.toml` with fields this kenn rejects
- **AND** every other kenn command fails while loading it
- **THEN** `kenn init -w <repo>` warns about the unparseable config and still runs
- **AND** `kenn init -w <repo> --force` backs it up and replaces it with a freshly detected config
- **AND** the report states that non-language settings could not be preserved
- **AND** subsequent commands against that workspace succeed

