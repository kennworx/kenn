## MODIFIED Requirements

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

