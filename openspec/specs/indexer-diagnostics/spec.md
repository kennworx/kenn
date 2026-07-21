# indexer-diagnostics Specification

## Purpose
kenn surfaces a failing indexer's own diagnostic — at the `kenn init` probe and during an index run — instead of a generic message, and distinguishes an absent indexer from a present-but-failing one.

## Requirements
### Requirement: kenn relays a failing indexer's own diagnostic

kenn SHALL surface a failing indexer's stderr to whoever invoked it, at both
moments an indexer can fail: the `kenn init` probe, and an index run.

kenn SHALL NOT discard that output in favour of a generic message. The failing
process — or the dynamic loader that refused to start it — names the specific
problem; a static hint can only name the tool, and telling a user to install
something already installed sends them the wrong way.

#### Scenario: A failing probe surfaces what the indexer said

- **WHEN** `kenn init` probes an indexer that executes and exits non-zero,
  writing an explanation to stderr
- **THEN** the report contains that explanation
- **AND** it is not replaced by the static per-language hint

#### Scenario: A binary that cannot start surfaces the loader's message

- **WHEN** an indexer cannot start because a linked library cannot be resolved
- **THEN** the report contains the loader's message naming that library
- **AND** the report does not claim the language's toolchain is missing

#### Scenario: A failing index run surfaces the indexer's message

- **WHEN** an indexer fails during `kenn index`
- **THEN** the run report's failure entry leads with the indexer's `error:`
  line rather than with unstructured output

### Requirement: kenn distinguishes an absent indexer from a failing one

kenn SHALL report an indexer that could not be executed differently from one
that executed and failed.

The fixes differ — install the indexer, versus the indexer is present but
something it needs is not — and a single "not runnable" message serves neither.

#### Scenario: The indexer is not installed

- **WHEN** a probe cannot execute the configured command at all
- **THEN** the report states the indexer was not found
- **AND** the static per-language install hint is shown, because there is no
  message from the indexer to prefer

#### Scenario: The indexer is installed but fails

- **WHEN** a probe executes the command and it exits non-zero
- **THEN** the report states the indexer ran and failed
- **AND** shows what it wrote to stderr

### Requirement: The extracted line leads, and the surrounding output is kept

Where kenn extracts a summary line from an indexer's stderr, it SHALL place
that line first and SHALL retain the surrounding output after it.

Two readers consume this and want different things: an agent reading a failure
over a tool call needs the actionable sentence at the front, and a person
debugging a broken toolchain needs the build output around it. Discarding
either one trades one information loss for another.

#### Scenario: A failure entry with build noise around the cause

- **WHEN** an indexer emits progress output, then an `error:` line, then a
  stack trace, and exits non-zero
- **THEN** the failure entry begins with the `error:` line
- **AND** the remaining output is still present in the entry
- **AND** the entry does not begin with a trailing stack-trace frame
