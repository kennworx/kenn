## MODIFIED Requirements

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
