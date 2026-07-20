## ADDED Requirements

### Requirement: Multiple JSONL producers with isolated id partitions

The pipeline SHALL support more than one registered `JsonlIndexer` (today: `kenn-dotnet` for C# and `kenn-ts` for TypeScript), invoking each at most once per workspace per run. Each JSONL producer's stream SHALL be ingested under its own `IdRegistry` (its own language partition), so producer-assigned `Ref`s from different producers never collide. The per-workspace single-invocation and indexer-owns-discovery contracts apply to every registered producer independently.

#### Scenario: C# and TypeScript producers coexist

- **WHEN** a workspace has both C# (`.sln`/`.csproj`) and TypeScript (`tsconfig.json`) sources and both languages are enabled
- **THEN** the pipeline invokes `kenn-dotnet` once and `kenn-ts` once
- **AND** each stream is ingested in its own id partition, with no `Ref` collision between the two

#### Scenario: TypeScript producer registered in place of the SCIP driver

- **WHEN** the runner is configured for TypeScript
- **THEN** `kenn-ts` is registered as a `JsonlIndexer` and no `scip-typescript` `ScipDriver` is registered

### Requirement: JSONL producers may be implemented in any language

A `JsonlIndexer` SHALL be free to be implemented in any host language and distributed as any executable form, provided it conforms to the JSONL wire and the invocation contract. `kenn-dotnet` is a self-contained .NET single-file binary; `kenn-ts` is a `bun build --compile` single-file executable embedding the TypeScript compiler. The pipeline treats both uniformly as spawned processes streaming frames on stdout.

#### Scenario: Compiled TypeScript producer is spawned like the C# producer

- **WHEN** the pipeline runs the TypeScript producer
- **THEN** it spawns the `build/kenn-ts` executable and ingests its stdout JSONL frame-by-frame, identically to how it spawns `build/kenn-dotnet`
