## ADDED Requirements

### Requirement: A kenn-authored indexer that cannot run says why, and how to fix it

An indexer kenn ships — `kenn-ts`, `kenn-dotnet`, `kenn-swift` — SHALL, when it
cannot run, write to **stderr** a line beginning with `error:` that names both
the missing dependency and the command that installs it, then exit non-zero.

The message SHALL describe the failure actually detected. An indexer SHALL NOT
name a dependency it has not determined to be missing: a specific, confident,
wrong instruction is worse than a generic one, because the user follows it.

Diagnostics SHALL NOT be written to stdout. Stdout carries the JSONL wire, and
a diagnostic there corrupts a frame and is reported as a malformed-output bug
in the indexer rather than as the missing dependency it is.

#### Scenario: The Swift indexer cannot resolve its index-store library

- **WHEN** `kenn-swift` runs on a machine where `libIndexStore` cannot be
  resolved
- **THEN** it writes an `error:` line to stderr naming that library
- **AND** the line includes a command that installs it
- **AND** the exit status is non-zero
- **AND** stdout carries no diagnostic output

#### Scenario: A diagnostic never reaches the wire

- **WHEN** any kenn-authored indexer emits a diagnostic
- **THEN** stdout contains only JSONL frames or nothing at all

### Requirement: The first `error:` line is the summary

The first `error:` line SHALL stand alone as a complete summary of the failure
and its fix, even where the indexer emits several lines of diagnostic around
it.

Consumers select this line out of surrounding build noise. Taking the last line
instead is not viable: tool failures commonly end in a backtrace frame, so the
final line is an implementation detail and the cause is mid-stream.

#### Scenario: A diagnostic surrounded by noise

- **WHEN** an indexer emits progress output, then an `error:` line, then a
  stack trace
- **THEN** the reported reason is the `error:` line
- **AND** it is not the trailing stack-trace frame

### Requirement: kenn relays the indexer's diagnostic to its caller

kenn SHALL surface a failing indexer's diagnostic to whoever invoked it, at
both moments an indexer can fail: the `kenn init` probe, and an index run.

kenn SHALL NOT discard a failing indexer's stderr. Replacing a specific,
first-hand explanation with a generic one is the failure this requirement
exists to prevent.

#### Scenario: A failing probe surfaces the indexer's message

- **WHEN** `kenn init` probes an indexer that fails with a diagnostic
- **THEN** the report contains that diagnostic

#### Scenario: A failing index run surfaces the indexer's message

- **WHEN** an indexer fails during `kenn index`
- **THEN** the run report's failure reason is the indexer's `error:` line

### Requirement: The diagnostic contract is enforced by a test

The sidecar probe suite SHALL assert that each kenn-authored indexer, run
against an unreachable toolchain, emits a conforming `error:` line naming an
install command.

The contract spans four codebases — Rust, C#, TypeScript and Swift — with no
shared type to enforce it. Absent this test, an indexer that stops conforming
is discovered by a user rather than by CI.

#### Scenario: A sidecar that stops emitting a diagnostic fails the suite

- **WHEN** a kenn-authored indexer exits non-zero against an unreachable
  toolchain without an `error:` line naming an install command
- **THEN** the probe suite fails and names that indexer
