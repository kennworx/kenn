## Context

`crates/kenn-indexer/src/driver.rs` defines a single trait
`LanguageDriver` with three methods:

```rust
pub trait LanguageDriver: Send + Sync {
    fn language_id(&self) -> &str;
    fn discover_units(&self, ws: &Workspace) -> Result<Vec<Unit>, DriverError>;
    fn run(&self, unit: &Unit, ws: &Workspace) -> Result<DriverOutcome, DriverError>;
}
```

`pipeline.rs::run_pipeline` calls `discover_units` then loops:

```rust
for unit in units {
    let outcome = driver.run(&unit, &workspace)?;
    match outcome { Scip { .. } | Jsonl { .. } | Unavailable { .. } => ... }
}
```

There are two real driver shapes living under this trait:

- **SCIP-style**: produces a `.scip` file per unit. Per-unit invocation
  is natural (rust-analyzer indexes one crate at a time, scip-typescript
  one tsconfig at a time). The pipeline ingests the file after the
  process exits.
- **JSONL-style** (`KennDotnet`): produces a streamed JSONL output. The
  underlying `kenn-dotnet` binary already accepts multiple `.sln` paths
  per invocation and `IndexerCore.RunCoreAsync` already iterates over
  them. Yet the trait forces one process per `.sln`.

Concrete cost of the mismatch on app (3 `.sln` files):
- Per-invocation startup: MSBuildLocator probe + JIT + workspace create
  ≈ 1-2s × 3 = 3-6s of overhead.
- Per-invocation metadata-reference cache: each kenn-dotnet process
  re-loads BCL/runtime/NuGet refs from disk. No cross-.sln sharing.
- Cold-start metadata-load bursts trigger Roslyn 4.7
  AccessViolationException races (observed empirically at
  `AnalyzerFileReference.GetExtensionTypeNameMap` and
  `SourceText.From` under parallel project loading).

## Goals / Non-Goals

**Goals.**

- Stop forcing kenn-dotnet through a per-unit invocation contract.
- Move project-list and scheduling decisions from the Rust orchestrator
  into kenn-dotnet, where the indexer can decide the optimal strategy.
- Preserve the SCIP-driver per-unit contract — that shape is correct
  for SCIP indexers.
- Keep the kenn-cli binary surface, JSON event stream, kenn.toml
  schema, and JSONL wire format unchanged.

**Non-Goals.**

- Cross-.sln `MSBuildWorkspace` reuse on the C# side. The C#
  `RunCoreAsync` today does `using var ws = MSBuildWorkspace.Create()`
  per `.sln`. Lifting that out is a follow-up that will give the bigger
  cache-sharing win once this trait split lands. Keeping it out of
  scope here so the Rust refactor can ship without C# coupling.
- AVE-crash mitigations. Lower MaxParallelism, retry-on-AVE, or
  source-generator detection are separate work; this change just makes
  them easier to land by reducing cold-start frequency.
- Daemon-mode kenn-dotnet. Bigger architectural change with its own
  tradeoffs.
- Cross-snapshot id stability (already a stated non-goal of the
  pipeline).

## Decisions

### Decision 1: Two distinct traits, not one parameterized trait

Define two separate traits in `driver.rs`:

```rust
pub trait ScipDriver: Send + Sync {
    fn language_id(&self) -> &str;
    fn discover_units(&self, ws: &Workspace) -> Result<Vec<Unit>, DriverError>;
    fn run_unit(&self, unit: &Unit, ws: &Workspace)
        -> Result<ScipOutcome, DriverError>;
}

pub trait JsonlIndexer: Send + Sync {
    fn language_id(&self) -> &str;
    fn run(&self, ws: &Workspace) -> Result<JsonlOutcome, DriverError>;
}
```

Alternative considered: one trait with an associated `enum DriverKind {
PerUnit, WorkspaceWide }` and a fat `run` method. Rejected — forces
every call site to handle both arms; more downcasting; doesn't
communicate intent.

`Unit` stays as the SCIP-side type. `JsonlOutcome` and `ScipOutcome`
replace `DriverOutcome`, with `Unavailable` lifted into each (SCIP and
JSONL indexers can both fail to find their binary).

### Decision 2: `IndexerDriver` holds two parallel vecs

```rust
pub struct IndexerDriver {
    pub workspace: Workspace,
    pub scip_drivers: Vec<Box<dyn ScipDriver>>,
    pub jsonl_indexers: Vec<Box<dyn JsonlIndexer>>,
}
```

Alternative considered: a single `Vec<Box<dyn AnyDriver>>` with a
discriminator. Same downcasting problem. Two vecs makes the
"pipeline branches" semantics explicit and lets us add more JSONL
indexers later (TS, Python via similar streaming model) without
reshaping.

### Decision 3: `KennDotnet::run` spawns one process with N `--projects`

```rust
fn run(&self, ws: &Workspace) -> Result<JsonlOutcome, DriverError> {
    let mut cmd = Command::new(&self.binary_path);
    cmd.arg("index").arg("--workspace").arg(ws.root());
    for sln in &self.projects {
        cmd.arg("--projects").arg(sln);
    }
    if self.skip_restore { cmd.arg("--skip-restore"); }
    // spawn, return JsonlOutcome
}
```

`KennDotnet` reads its `projects` list from `kenn.toml` via the
existing config layer (no schema change). If the list is empty, it
falls back to discovering `.sln`/`.csproj` under the workspace — the
same logic that lives today in `discover_units`, just relocated.

`kenn-dotnet`'s System.CommandLine option already declares
`AllowMultipleArgumentsPerToken = true` and `Arity =
ArgumentArity.ZeroOrMore`; passing N `--projects` arguments works
today.

### Decision 4: One `RunReport` per JSONL invocation

Today every unit produces a `RunReport`. With JSONL collapsed into one
invocation per indexer, we emit one report covering all `.sln`s. The
report's `unit_identifier` becomes a synthetic value (e.g.
`"kenn-dotnet[3 slns]"` or the workspace root path).

Per-`.sln` error attribution is not lost: kenn-dotnet emits
`ErrorFrame.path = <sln path>` for msbuild failures, and the
`failed_projects` field on the report is populated from those frames
during ingestion. The user-facing per-`.sln` error visibility through
`kenn status` continues to work.

### Decision 5: Pipeline branches, retry logic moves with the JSONL path

`run_pipeline` becomes:

```rust
for driver in &runner.scip_drivers {
    let units = driver.discover_units(&workspace)?;
    for unit in units { /* existing per-unit ingest */ }
}
for indexer in &runner.jsonl_indexers {
    let outcome = indexer.run(&workspace)?;
    /* single-invocation ingest */
}
```

`run_jsonl_with_retry` migrates to the JSONL branch. Its `unit:
&Unit` parameter goes away; retry attribution uses the workspace path
+ indexer language_id.

## Risks / Trade-offs

- **RunReport plurality changes** → user-facing report list shrinks
  (3 reports → 1 for kenn-dotnet on app). Mitigation: per-.sln
  error frames still attributed via `failed_projects` populated
  from ErrorFrame.path; tests cover the new shape.

- **Out-of-tree consumers of `LanguageDriver`** → none known, but
  if they exist they'll break. Mitigation: trait rename is loud at
  compile time; no silent behavior change.

- **Discovery semantics moving from Rust to C#** when `projects` in
  kenn.toml is empty → today Rust globs the workspace and picks
  `.sln` (preferred) or `.csproj`. After this change kenn-dotnet's
  `SolutionLoader.DiscoverProjectFiles` does the equivalent. Two
  implementations could diverge.
  Mitigation: the existing C# discovery already mirrors the Rust
  rule (prefer `.sln`, exclude `bin/`/`obj/`). Add a regression
  test that compares the two for sample workspaces; lock the
  contract in the new spec.

- **Test surface** → existing pipeline tests stub a `LanguageDriver`.
  They need to split into ScipDriver and JsonlIndexer stubs.
  Mitigation: keep both stubs simple; expand fixture set rather
  than reuse one stub for both shapes.

- **Speedup is modest** → ~5% wall-clock saving on app. Real
  reason to do this is architectural clarity + setting up the
  follow-on changes (cross-.sln workspace reuse, AVE mitigations).
  We should not oversell the perf win.

## Migration Plan

1. Land the trait split + `KennDotnet` refactor + pipeline branch in
   one PR. Internal API only — no on-disk migration.
2. Update `kenn-cli/src/cmd_index.rs` to register `KennDotnet` as a
   `JsonlIndexer`.
3. Update tests (split stubs, adjust per-unit expectations to
   per-invocation where applicable).
4. Run `kenn index --force --json` against app — confirm
   wall-clock improvement, snapshot equivalence, no AVE-class
   regressions.
5. No rollback hatch needed beyond `git revert`; the change does not
   touch on-disk state, kenn.toml, or wire format.

## Open Questions

- Should the new spec live as a sibling of `dotnet-stream-indexer` (the
  wire-format spec) or under a broader `pipeline` capability that also
  covers SCIP drivers? Current plan: new sibling spec
  `jsonl-indexer-driver`. The pipeline-wide orchestration concern is
  shared but the contracts (per-unit vs workspace-wide) are different
  enough to keep separate.
- Should `JsonlIndexer::run` take `&[Unit]` for symmetry, even if
  KennDotnet ignores the list and reads from kenn.toml? Current plan:
  no — the whole point is to take the indexer out of the per-unit
  shape. Workspace + its own config is the contract.
