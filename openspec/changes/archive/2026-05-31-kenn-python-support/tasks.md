> **Sequencing note**: §1–§4 form one indivisible commit. Between renaming the driver structs (§1) and updating the wire-up (§4), the workspace will not compile because `cmd_index.rs` / `workflow.rs` still reference the old `kenn_dotnet_path` / `rust_analyzer_path` / `kenn_ts_path` fields. Hold all four sections in a single working-tree pass before running clippy / tests. §5 (MCP) and §6 (verify) can land separately.

## 1. Uniform `command: Vec<String>` shape across existing drivers

- [x] 1.1 In `crates/kenn-indexer/src/driver/rust.rs`, replace `RustAnalyzer::binary_path: Option<PathBuf>` with `command: Vec<String>` (default `vec!["rust-analyzer".into()]`); update `command()` to `PathBuf::from(&self.command[0])`; update the `Command::new(...)` site to `Command::new(&self.command[0]).args(&self.command[1..])` then chain existing args.
- [x] 1.2 Same rename in `crates/kenn-indexer/src/driver/typescript.rs` (`KennTs::binary_path` → `command`, default `vec!["kenn-ts".into()]`).
- [x] 1.3 Same rename in `crates/kenn-indexer/src/driver/dotnet.rs` (`KennDotnet::binary_path` → `command`, default `vec!["kenn-dotnet".into()]`).
- [x] 1.4 Update the three `*_returns_unavailable_when_binary_missing` tests in `crates/kenn-indexer/src/driver/mod.rs` to construct the new `command: vec![...]` form.

## 2. Uniform `[language.*]` config shape

- [x] 2.1 In `crates/kenn-config/src/lib.rs`, replace `CsharpConfig::kenn_dotnet_path: Option<PathBuf>` with `command: Vec<String>` (default `vec!["kenn-dotnet".into()]`); flip `CsharpConfig::enabled` default from `true` → `false`.
- [x] 2.2 Replace `RustConfig::rust_analyzer_path` with `command: Vec<String>` (default `vec!["rust-analyzer".into()]`).
- [x] 2.3 Replace `TypescriptConfig::kenn_ts_path` with `command: Vec<String>` (default `vec!["kenn-ts".into()]`).
- [x] 2.4 Add `PythonConfig { enabled: bool (default false), command: Vec<String> (default ["scip-python"]), project_name: Option<String>, project_version: Option<String> }`, all fields `#[serde(default)]`, struct `#[serde(deny_unknown_fields)]`.
- [x] 2.5 Add `pub python: PythonConfig` to `LanguageConfig`.
- [x] 2.6 Add config validation: reject `command = []` for any language at config load (clear error naming the offending language).
- [x] 2.7 Update the existing `cmd_status` / config-loading tests for the renamed fields and the C# `enabled` default flip; add a test for the empty-`command` rejection.

## 3. New `ScipPython` driver

- [x] 3.1 Create `crates/kenn-indexer/src/driver/python.rs` with `pub struct ScipPython { command: Vec<String>, project_name: Option<String>, project_version: Option<String> }` and `impl ScipDriver for ScipPython`.
- [x] 3.2 `language_id()` returns `"python"`; `command()` returns `PathBuf::from(&self.command[0])`.
- [x] 3.3 `discover_units` walks via `super::walk_skipping` with extra skip leaves `["__pycache__", ".venv", "venv", "node_modules", ".kenn"]` on top of the existing `.git`/`bin`/`obj`; return one unit `{ identifier: "python", path: workspace.root() }` iff a `.py` or `.pyi` file is found, else empty.
- [x] 3.4 `run_unit` allocates `make_scip_output_path(workspace, "python")`, spawns `Command::new(&self.command[0]).args(&self.command[1..]).args(["index", "--cwd", ws_root, "--output", out, "--quiet"])`, optionally appends `--project-name`/`--project-version` when set, captures stderr; on success returns `ScipOutcome::Scip { path, report }`; on `NotFound` returns `ScipOutcome::Unavailable` with a "scip-python launcher not found" report message.
- [x] 3.5 Register the module in `crates/kenn-indexer/src/driver/mod.rs` (`mod python; pub use python::ScipPython;`).
- [x] 3.6 Add three driver unit tests in `mod.rs`: `scip_python_discovers_one_unit_when_py_file_present`, `scip_python_discovers_no_units_without_py_files`, `scip_python_returns_unavailable_when_binary_missing`.
- [x] 3.7 Add a test that `.py` files only under `.venv/`, `__pycache__/`, `node_modules/`, or `.kenn/` produce zero units.

## 4. Wire-up at both driver-construction sites

- [x] 4.1 In `crates/kenn-cli/src/cmd_index.rs`, add `ScipPython` to the `use kenn_indexer::driver::{...}` import; in `build_driver`, replace the four bespoke branches with a uniform form: one `if config.language.<lang>.enabled { runner = runner.with_*_driver(<Driver> { command: cfg.command.clone(), ... }) }` per language, including a Python branch.
- [x] 4.2 Same wire-up update in `crates/kenn-indexer/src/workflow.rs`.
- [x] 4.3 Update `kenn index` user-facing log when zero languages are enabled to print "no languages enabled; nothing to index — see kenn.toml" so the case is discoverable.
- [x] 4.4 Update `crates/kenn-cli/src/starter_kenn.toml` written by `kenn init`. Keep the file scannable: flip `[language.csharp] enabled = true` → `enabled = false`; replace the commented `# kenn_dotnet_path = ...` line with a single commented `# command = ["kenn-dotnet"]`. Add `[language.rust]`, `[language.typescript]`, `[language.python]` blocks following the csharp pattern — `enabled = false` (uncommented so the field is visible), one commented default `# command = [...]`, and one comment line per block. Under `[language.python]` only, add one extra comment: `# Other runtimes: bunx, npx, uvx — see docs/`. Do not list every alternative inline.

## 5. MCP empty-snapshot config diagnostics

- [x] 5.1 In `crates/kenn-mcp/src/tools.rs` (which already holds `kenn_config::Config`, line 44), add a helper `empty_snapshot_hint(config, snapshot_symbol_count) -> Option<ConfigHint>` returning `Some(ConfigHint { kind: ConfigDisabled, enabled_languages: vec![] })` when symbols=0 and every `[language.*].enabled` is false, `Some(ConfigHint { kind: ConfiguredButEmpty, enabled_languages: <names> })` when symbols=0 and ≥1 enabled, `None` when symbols > 0. The workspace whose Config is consulted MUST be the one already resolved by the workspace-resolution chain — do not re-resolve from cwd.
- [x] 5.2 In `crates/kenn-mcp/src/error.rs`, add a new `McpErrorCode::EmptySnapshot` variant whose `as_str()` returns `"EMPTY_SNAPSHOT"` and whose numeric JSON-RPC code is `-32002` (reusing the existing service-unavailable code alongside `IndexUnavailable`/`EmbedderStarting`). Thread the hint into every data-returning MCP tool (per the spec's enumerated list, excluding `get_index_status` and `get_workspace_overview`): when the helper returns `Some(hint)`, return a structured JSON-RPC error using the new variant whose `message` matches the spec's wording and whose `data` carries `{ code: "EMPTY_SNAPSHOT", kind, enabled_languages }`.
- [x] 5.3 Extend `get_workspace_overview`'s response struct with `config_hint: Option<ConfigHint>` populated from the same helper. `None` MUST serialize as omitted or `null` (not as a present-but-empty object) so healthy-snapshot responses stay backwards-compatible.
- [x] 5.4 Leave `get_index_status` untouched — it must remain a lifecycle-only probe.
- [x] 5.5 Add tests: (a) empty snapshot + all-disabled config → tool error `code = -32002`, `data.code = "EMPTY_SNAPSHOT"`, `data.kind = "config-disabled"`, `data.enabled_languages = []`, message mentions `kenn.toml` and the literal strings `csharp`, `rust`, `typescript`, `python`; (b) empty snapshot + only-Python-enabled config → tool error `code = -32002`, `data.code = "EMPTY_SNAPSHOT"`, `data.kind = "configured-but-empty"`, `data.enabled_languages = ["python"]`, message names Python; (c) `get_workspace_overview` on empty snapshot carries the right `config_hint`; (d) `get_workspace_overview` on a healthy snapshot omits / nulls `config_hint`.

## 6. Verification — quality gates and end-to-end

- [x] 6.1 `cargo clippy --workspace --all-targets` — zero warnings (per CLAUDE.md §5); add narrow `#[allow]`s only where pedantic flags are intentional.
- [x] 6.2 `cargo test -p kenn-indexer -p kenn-config -p kenn-cli -p kenn-mcp` — new tests pass; existing tests still pass after the field renames.
- [x] 6.3 `just crap-ci` — no regression / no new over-threshold function on the new driver / MCP-hint modules; refresh baseline only if pre-existing entries shift.
- [x] 6.4 Build with `cargo build -p kenn-cli` and run `build/kenn index` against `tmp/graphify` after writing a `kenn.toml` with `[language.python] enabled = true` and `command = ["bunx", "@sourcegraph/scip-python"]`; tee to `tmp/graphify-py-index.log`.
- [x] 6.5 `build/kenn status` against `tmp/graphify` reports `documents ≈ 91` (path canonicalization may split or merge), `definitions ≈ 11978`, `symbols` populated, `status = ok`, no failed projects. Edges are expected non-zero (scip-python emits no SCIP `Relationship` records — verified on the graphify `.scip` — so kenn-side edges come from FROM-attributed `ReadAccess` occurrences using scip-python's populated `Occurrence.enclosing_range`). Record the actual edge count in the commit message; if it is unexpectedly 0, treat as a Python-transform bug, not a verification failure.
- [x] 6.6 Through the kenn MCP tools (ask user to reload kenn-mcp first per the memory note), confirm `get_workspace_overview` lists Python and `search_symbols("ingest")` returns `py:` results from graphify.
- [x] 6.7 Negative-path verification of §5: against an empty workspace with no `kenn.toml` overrides AND either `--force` indexing or `staleness.git_aware_skip = false` (so the prepare phase doesn't no-op on staleness-skip), confirm `kenn mcp`'s `search_symbols` call returns the structured error with `code = -32002`, `data.code = "EMPTY_SNAPSHOT"`, `data.kind = "config-disabled"`, and the message referencing `kenn.toml`.
- [x] 6.8 `cargo fmt --all` as the final pre-commit step (per CLAUDE.md §7).
