## Why

The Rust pipeline today drives `kenn-dotnet` through the same generic
`LanguageDriver` trait used for SCIP-producing indexers (rust-analyzer,
scip-typescript, etc.). That trait is per-unit: `discover_units(ws)` →
`run(unit, ws)` is called in a loop, and `kenn-cli` invokes
`kenn-dotnet` separately for each `.sln`.

The shape doesn't fit kenn-dotnet:

1. **Per-.sln startup tax.** Each invocation pays MSBuildLocator probe,
   MSBuildWorkspace.Create, BuildHostGuard install, JIT warmup — order of
   1-2s × N .slns. On app (3 .slns) that's ~5s of pure overhead.
2. **No cross-.sln cache sharing.** Each kenn-dotnet process has its own
   metadata-reference cache, so shared BCL/runtime/NuGet refs are loaded
   N times.
3. **Roslyn 4.7 concurrency hazards.** Cold metadata loading bursts
   trigger AccessViolationExceptions in `AnalyzerFileReference` and
   `SourceText.From` under parallelism. A single long-lived workspace
   amortizes the cold-start burst across .slns and reduces the chance of
   tripping these races.
4. **Scheduling decisions made at the wrong layer.** Whether to walk
   .slns sequentially, batch them, or share state belongs to the indexer
   that knows its own internals — not to the Rust orchestrator.

## What Changes

- **NEW** trait `JsonlIndexer { fn run(&self, ws: &Workspace) ->
  JsonlOutcome }` in `crates/kenn-indexer/src/driver.rs`. One call per
  workspace; the indexer decides internally what to index and how to
  schedule it.
- `KennDotnet` becomes the one `JsonlIndexer` impl. It holds the
  configured `projects: Vec<PathBuf>` from `kenn.toml` and, on `run`,
  spawns ONE `kenn-dotnet index --workspace <ws> --projects <a> <b> <c>`
  process.
- The existing `LanguageDriver` trait is renamed to `ScipDriver` and
  retains its per-unit shape (`discover_units` + `run_unit`). SCIP
  drivers (currently the stub) keep working unchanged.
- `IndexerDriver` gains a parallel container: `scip_drivers:
  Vec<Box<dyn ScipDriver>>` and `jsonl_indexers: Vec<Box<dyn
  JsonlIndexer>>`.
- `run_pipeline` branches: SCIP drivers take the per-unit path; JSONL
  indexers take a single-invocation path. The retry-on-AVE logic
  migrates to the JSONL path and loses its `unit` parameter.
- `RunReport` granularity moves from per-unit to per-invocation for
  JSONL indexers. Per-.sln error attribution still flows through
  `ErrorFrame.path` in the wire stream — that visibility is preserved.
- **BREAKING (internal Rust API)**: trait names and pipeline signatures
  change. The kenn-cli binary surface and kenn.toml schema are
  unchanged.

## Capabilities

### New Capabilities

- `jsonl-indexer-driver`: defines the contract by which the kenn-cli
  pipeline invokes streaming-JSONL indexers (today: kenn-dotnet). One
  process per workspace; indexer owns project discovery and scheduling.

### Modified Capabilities

None. The wire format spec (`dotnet-stream-indexer`) is unchanged — this
change is purely about how the Rust pipeline drives the indexer
process.

## Impact

- **Code**:
  - `crates/kenn-indexer/src/driver.rs` — split trait, refactor
    `KennDotnet` impl, update `IndexerDriver` struct.
  - `crates/kenn-indexer/src/pipeline.rs` — branch by driver kind in
    `run_pipeline`; move `run_jsonl_with_retry` off the unit-loop path.
  - `crates/kenn-indexer/src/lib.rs` — re-export the new trait.
  - `crates/kenn-cli/src/cmd_index.rs` — register `KennDotnet` as a
    `JsonlIndexer` instead of a `LanguageDriver`.
  - Test fixtures: `StubJsonlIndexer` for new trait; existing
    `StubScipDriver` stays.
- **APIs**: kenn-cli binary surface, JSON event stream, and
  kenn.toml schema are unchanged. Internal Rust trait + pipeline API
  changes are breaking for any out-of-tree consumer (none today).
- **Schema**: no changes.
- **C# side**: zero code changes. `kenn-dotnet --projects A B C` is
  already supported; `IndexerCore.RunCoreAsync` already iterates over
  multiple `.sln` entries. The C# multi-.sln loop becomes the
  consolidation point.
- **Performance**: removes 3-5s of per-.sln startup overhead on app
  (~5% of total wall). Sets the table for follow-up work
  (cross-.sln MSBuildWorkspace reuse, AVE-crash mitigations,
  daemon mode) — but those are out of scope here.
