## ADDED Requirements

### Requirement: Cold start does not serve an empty snapshot for a configured workspace

The cold-start startup decision SHALL NOT skip to (serve as `Ready`) a
retained snapshot that contains **zero symbols** when the active
`kenn.toml` has **at least one language enabled**. In that case the
server SHALL re-index instead — it remains in `Indexing` (data tools
fail fast with `INDEX_UNAVAILABLE`) until the re-index completes, rather
than presenting an empty `Ready` snapshot that an agent would misread as
"the index is not built."

This refines the snapshot-freshness skip rule: a matching `StalenessKey`
is necessary but no longer sufficient to skip — a zero-symbol snapshot
under a language-enabled config is treated as not serviceable. This
recovers the common case where a prior index run produced zero symbols
because of a transient indexer failure (a language server failed to
launch) and published the empty result under the workspace's key: the
next cold start re-indexes rather than serving the stale empty snapshot
indefinitely.

A workspace that legitimately yields no symbols SHALL still settle to
`Ready` and SHALL NOT cause a per-startup re-index loop:

- When no `kenn.toml` exists, or every `[language.*].enabled` is false,
  the config does not expect symbols; the server SHALL settle to `Ready`
  on the empty snapshot and surface the existing empty-snapshot
  config-hint (`not-initialized` / `config-disabled`).
- When at least one language is enabled but the re-index again produces
  zero symbols, the server SHALL settle to `Ready` on that freshly-built
  empty snapshot and surface the `configured-but-empty` hint. The
  re-index runs at most once per cold start; the server does not loop.

#### Scenario: Empty snapshot under enabled language triggers re-index

- **GIVEN** a retained snapshot whose `StalenessKey` matches the
  workspace but which contains zero symbols
- **AND** `kenn.toml` enables at least one language
- **WHEN** the MCP server starts
- **THEN** the server does NOT serve the empty snapshot as `Ready`
- **AND** the server enters `Indexing` and re-runs the pipeline

#### Scenario: Re-index now produces symbols

- **GIVEN** the empty-snapshot re-index path is taken
- **AND** the indexer now succeeds (the prior emptiness was a transient
  failure)
- **WHEN** the re-index completes
- **THEN** the server transitions to `Ready` on a populated snapshot

#### Scenario: Genuinely empty configured workspace settles without looping

- **GIVEN** a workspace with a language enabled but no matching source
  files
- **WHEN** the MCP server starts and the cold-start re-index produces an
  empty snapshot
- **THEN** the server settles to `Ready` on that snapshot with the
  `configured-but-empty` config-hint
- **AND** the server does NOT immediately re-index again

#### Scenario: Unconfigured workspace settles to Ready without re-index

- **GIVEN** a workspace with no `kenn.toml` (or all languages disabled)
- **WHEN** the MCP server starts with an empty live snapshot
- **THEN** the server settles to `Ready` and surfaces the
  `not-initialized` / `config-disabled` hint
- **AND** the server does NOT trigger a re-index on account of the empty
  snapshot
