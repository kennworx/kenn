## 1. Trait split

- [x] 1.1 Rename `LanguageDriver` → `ScipDriver` in
      `crates/kenn-indexer/src/driver.rs`. Rename its `run` →
      `run_unit` (per-unit semantics now explicit in the name).
- [x] 1.2 Define new trait `JsonlIndexer { fn language_id(&self) ->
      &str; fn run(&self, ws: &Workspace) -> Result<JsonlOutcome,
      DriverError>; }` in the same file. Keep
      `ScipDriver::discover_units` as-is.
- [x] 1.3 Split `DriverOutcome` into `ScipOutcome` (Scip + Unavailable
      arms) and `JsonlOutcome` (Jsonl + Unavailable arms). Both keep
      a `RunReport` field. Drop the unified enum.
- [x] 1.4 Remove `Unit` requirements from `JsonlIndexer`'s API
      surface. Keep `Unit` for SCIP only.

## 2. KennDotnet refactor

- [x] 2.1 `KennDotnet` gains a `projects: Vec<PathBuf>` field
      populated from `kenn.toml`'s `[language.csharp].projects`.
      `KennDotnet::default()` keeps its empty-default behaviour;
      construction in `cmd_index.rs` reads the configured list.
- [x] 2.2 Implement `JsonlIndexer for KennDotnet`. The `run` method
      builds one `Command` with `index --workspace <ws>`, repeated
      `--projects <sln>` for each configured path, optional
      `--skip-restore`, then spawns. Returns `JsonlOutcome::Jsonl
      { child, stdout, stderr, report }`.
- [x] 2.3 When `self.projects` is empty, fall back to the existing
      `discover_units` discovery logic inline (prefer `.sln` over
      `.csproj`, exclude `bin/`/`obj/`). After this change, that
      logic exists only inside `KennDotnet::run`; `discover_units`
      is removed from `KennDotnet`.
- [x] 2.4 Remove the `LanguageDriver`/`ScipDriver` impl from
      `KennDotnet`. It is no longer per-unit.

## 3. IndexerDriver + pipeline

- [x] 3.1 `IndexerDriver` struct gains
      `scip_drivers: Vec<Box<dyn ScipDriver>>` and
      `jsonl_indexers: Vec<Box<dyn JsonlIndexer>>` (replacing the
      current single `language_drivers` vec). `with_driver` becomes
      `with_scip_driver` / `with_jsonl_indexer` helpers.
- [x] 3.2 `run_pipeline` in `pipeline.rs` branches: first the SCIP
      drivers loop (discover_units → for each unit run_unit + ingest
      .scip), then the JSONL indexers loop (single run + streaming
      ingest of stdout). Per-driver-kind paths share the
      `IdRegistry`, `BatchingSink`, and finalize logic.
- [x] 3.3 `run_jsonl_with_retry` migrates to the JSONL branch. Drop
      its `unit: &Unit` parameter; retry attribution uses workspace
      path + indexer language_id.
- [x] 3.4 RunReport for a JSONL invocation: one report per indexer
      run, identifier set to a synthetic value
      (e.g. `format!("{}@{}", lang_id, ws.root().display())`).
      Verify per-`.sln` `failed_projects` still populates from
      `ErrorFrame.path` — the existing `transform_jsonl` ingest
      path already does this; make sure the move doesn't drop it.

## 4. cmd_index wiring

- [x] 4.1 `crates/kenn-cli/src/cmd_index.rs` constructs `KennDotnet`
      with the configured `projects` list and registers it via
      `IndexerDriver::with_jsonl_indexer`. SCIP drivers (none real
      today) would register via `with_scip_driver`.
- [x] 4.2 Confirm the kenn-cli binary surface (CLI args,
      `kenn index --json` event shape) is unchanged.

## 5. Tests

- [x] 5.1 Existing pipeline tests in `pipeline.rs` and
      `driver.rs` use a `StubScipDriver`. Confirm they still
      compile + pass under the renamed trait.
- [x] 5.2 Add `StubJsonlIndexer` to the test fixtures: returns a
      synthetic JSONL stream from a fixture string. Cover one
      pipeline test that exercises: streaming ingestion, single
      RunReport per invocation, per-`.sln` error attribution from
      `ErrorFrame.path`.
- [x] 5.3 Add a discovery-parity test: invoke `KennDotnet::run`
      with the configured `projects` list explicit, then with the
      list empty (forcing internal discovery), against a small
      fixture workspace; assert both produce the same set of
      `.sln` paths in the spawned command.
- [x] 5.4 `cargo clippy --workspace --all-targets` clean.

## 6. End-to-end validation

- [x] 6.1 Run `kenn index --force --json` against
      the production workspace (3 configured `.sln`s).
      Confirm:
        - Exit 0 (or partial-status if pre-existing msbuild errors
          are still present).
        - Snapshot stats: documents/symbols/edges counts within
          ±1% of the prior multi-invocation baseline.
        - One RunReport for kenn-dotnet (was 3).
        - Wall-clock improvement: target ~3-5s saved by removing
          per-`.sln` startup tax. Record the number.
- [x] 6.2 `pgrep -lf "kenn-dotnet|MSBuild|BuildHost"` shows zero
      survivors after the run.
- [x] 6.3 `cargo clippy --workspace --all-targets` zero warnings on
      the final state. `cargo test` passes.
