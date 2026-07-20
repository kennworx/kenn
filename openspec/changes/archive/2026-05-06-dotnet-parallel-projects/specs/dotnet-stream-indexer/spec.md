## ADDED Requirements

### Requirement: Per-project walks may run concurrently

The producer SHALL be permitted to walk projects within a single
`kenn-dotnet` invocation concurrently. The maximum concurrency is
controlled by the `--max-parallelism` CLI flag and defaults to
`Environment.ProcessorCount`. A value of `1` SHALL produce a strictly
serial walk equivalent to the pre-parallel behavior.

`MSBuildWorkspace.OpenSolutionAsync` and the project-load phase SHALL
remain serial; concurrency applies after `LoadProjectsAsync` has
returned the project list.

#### Scenario: Default parallelism uses processor count

- **WHEN** the user runs `kenn-dotnet index` without
  `--max-parallelism`
- **THEN** project walks dispatch to a worker pool sized to
  `Environment.ProcessorCount`
- **AND** wall-clock runtime on a multi-project workspace is lower
  than the equivalent `--max-parallelism 1` run

#### Scenario: max-parallelism 1 is bit-stable serial fallback

- **WHEN** the user runs `kenn-dotnet index --max-parallelism 1`
- **THEN** project walks happen one at a time
- **AND** the resulting JSONL output is identical to the pre-parallel
  serial behavior (modulo non-deterministic content like timestamps
  in `meta`)

### Requirement: Concurrent emission preserves ordering invariants

The producer SHALL preserve the introduce-before-reference invariant
even under concurrent emission: every `Ref` referenced by a frame's
`source`, `target`, `parent`, `pkg`, or `file` field MUST have been
introduced (as a `PackageFrame`, `FileFrame`, `StubFrame`, or
`SymbolFrame`) earlier in the stream. Concurrency does not relax
this contract.

Frame lines are atomic: each frame is fully serialized to one
JSON-encoded line before the next emission begins. Frames from
different workers MAY interleave at the line level — there is no
required cross-worker ordering — but no frame line is ever split or
truncated.

The single `meta` frame SHALL be emitted before any worker starts;
the single `end` frame SHALL be emitted after every worker has
completed.

#### Scenario: Concurrent writes do not corrupt JSONL framing

- **WHEN** `kenn-dotnet` runs with `--max-parallelism N` for any `N
  > 1`
- **THEN** every output line parses as a valid JSON object
- **AND** the line discriminator `type` field is one of the wire
  format's defined values

#### Scenario: References resolve under interleaving

- **WHEN** the consumer ingests the parallel-emitted stream
- **THEN** every `EdgeFrame.source` and `EdgeFrame.target` resolves
  to a previously-introduced symbol or file id
- **AND** every `SymbolFrame.parent`, `pkg`, and `file` resolves to
  a previously-introduced id
- **AND** the consumer-side counts (symbols, defs, edges) match those
  of an equivalent `--max-parallelism 1` run on the same workspace
