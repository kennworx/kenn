## Context

`crates/kenn-indexer/src/driver/mod.rs` defines two driver traits: `ScipDriver` (per-unit `.scip` producer; impls: `RustAnalyzer`, `KennDotnet` is the JSONL one) and `JsonlIndexer` (workspace-wide JSONL stream; impls: `KennDotnet`, `KennTs`). The transform pipeline already understands scip-python output — `PythonTransformer` (`crates/kenn-model/src/id/py.rs`) maps `scip-python python <dist> <ver> <descriptor>` symbols to `py:` IDs, `transform.rs` maps `.py`/`.pyi` paths to `Language::Python`, and the kind classifier handles scip-python's empty `SymbolInformation.kind` by descriptor-suffix inference. A manual scip-python run on `tmp/graphify` (91 `.py` files) produces a 4 MB `.scip` with 11 978 definitions / 57 822 occurrences — the data the pipeline needs is already producible; only the driver glue is missing.

Separately, the four language configs in `kenn-config` have drifted into inconsistent shapes: `CsharpConfig::enabled` defaults to `true` while `RustConfig::enabled` / `TypescriptConfig::enabled` default to `false`; each carries a bespoke `kenn_dotnet_path` / `rust_analyzer_path` / `kenn_ts_path: Option<PathBuf>` for binary override. Adding a fourth language is the right moment to converge them.

## Goals / Non-Goals

**Goals:**
- A `ScipPython` driver that fits the existing `ScipDriver` trait and produces `.scip` output the current transform pipeline ingests unchanged.
- Uniform `[language.*]` config shape: `enabled: bool` (default `false` everywhere) + `command: Vec<String>` (launcher tokens) across csharp, rust, typescript, python.
- A single, end-to-end successful index of `tmp/graphify` (Python-only) reporting populated documents / symbols / definitions / edges.

**Non-Goals:**
- Back-compat shim for the old `*_path` / csharp-enabled-by-default `kenn.toml` shape — the single known user is driving this change.
- Auto-resolution of bun/npm/pip launchers — kenn does not guess; the user puts the launcher tokens they want in `command`.
- Changes to `PythonTransformer`, the SCIP→record path, or any spec other than `scip-indexer`.
- Performance tuning (priority lowering, thread caps) for scip-python — can be added later if needed; rust-analyzer's analogs took multiple rounds to stabilise.

## Decisions

### `ScipPython` is a `ScipDriver`, not a `JsonlIndexer`

scip-python writes a `.scip` protobuf to disk (no JSONL streaming option). The `ScipDriver` trait — `discover_units` + `run_unit` returning `ScipOutcome::Scip { path, report }` — fits exactly, the same way `RustAnalyzer` does. The pipeline already handles streaming-parse of `.scip` files post-run. **Alternative considered**: bolt a JSONL-streaming layer onto scip-python via a wrapper. Rejected — scip-python emits everything at end-of-run anyway (it's a whole-workspace Pyright analysis), so streaming buys nothing and adds parser surface.

### Unit discovery: one unit per workspace iff any `.py` file exists

scip-python takes the whole workspace as input via `--cwd`; pointing it at a sub-package doesn't reduce work (Pyright loads the project graph regardless). Discovery returns `vec![Unit { identifier: "python", path: workspace.root() }]` when `walk_skipping` finds at least one `.py`, otherwise empty. Skip `__pycache__`, `.venv`, `venv`, `node_modules`, `.kenn` (plus the standard `.git`/`bin`/`obj` and the linked-worktree exclusions the walker already applies). **Alternative**: discover per-`pyproject.toml`. Rejected — many real Python repos have no `pyproject.toml` (graphify does, but lots don't), and scip-python doesn't benefit from multiple runs anyway.

### Launcher is a `Vec<String>`, not a `PathBuf`

The user runs Python tooling through different launchers — `bunx @sourcegraph/scip-python`, `npx --yes @sourcegraph/scip-python`, or a global `scip-python` install. The existing `*_path: Option<PathBuf>` shape forces a single executable, which doesn't fit a launcher-plus-package model. `command: Vec<String>` is the smallest change that handles all three: `command[0]` is the program (and what the phase-1 CLI-availability preflight checks), `command[1..]` is prepended to the existing arg list. **Alternative considered**: keep `*_path` and add a separate `launcher_args: Vec<String>`. Rejected — splits one concept across two fields.

This shape generalises to all four drivers, so the same change applies to `RustAnalyzer`, `KennTs`, `KennDotnet`. Defaults: `["rust-analyzer"]`, `["kenn-ts"]`, `["kenn-dotnet"]`, `["scip-python"]`.

### C# default flips to `enabled = false`

C# is the only language currently enabled by default, which surprises users on rust-only or Python-only repos (the dotnet driver runs and either succeeds vacuously or emits a "projects=0" failure, both noise). Aligning on opt-in matches the project context of "no features beyond what's asked": index nothing the user didn't enable. **Alternative**: keep C# default true and flip Python/rust/typescript to true as well. Rejected — that turns a missing scip-python install into a hard failure on every workspace.

### Spec deltas target `scip-indexer` and `mcp-server`

`scip-indexer`'s "Per-language indexer dispatch" requirement already names `scip-python index` as the Python dispatch target. So Python is in the contract; what's missing is scenarios (and the implementation behind them). The launcher-vector, uniform-enablement, and Python-discovery changes all land as `ADDED Requirements` in the `scip-indexer` delta.

The "no symbols → point at config" UX lives in `mcp-server` (it's an MCP-surface contract, not an indexer one). The delta there adds one requirement that pins the `config-disabled` vs `configured-but-empty` classification, the JSON-RPC `data` payload shape (`{ kind, enabled_languages }`), the dedicated error code (`-32010 MCP_NO_DATA`), and the `get_workspace_overview.config_hint` field. The new requirement is the empty-snapshot dual of the existing *An unresolved entity reference is an error, not an empty result* requirement — same design language ("structured error beats silent empty result"), applied at a different layer.

`kenn-mcp` already holds `kenn_config::Config` at `tools.rs:44`, so the empty-snapshot classifier is a plain helper — no MCP boundary plumbing. The `kenn.toml` consulted MUST be the one belonging to the workspace resolved by the existing *Workspace resolution follows a five-step priority chain* requirement (not `cwd`), so worktree-bound MCP sessions see the right config; the implementer reads from the already-resolved `Config`, not from disk.

**JSON-RPC code allocation.** The error reuses `-32002` (kenn-mcp's existing "service-unavailable" numeric code) with a new string code `EMPTY_SNAPSHOT` in `data.code`, alongside the existing `INDEX_UNAVAILABLE`/`EMBEDDER_STARTING` siblings. Reusing the numeric code matches the existing kenn-mcp convention (agents already branch on `data.code` strings, not numeric codes) and avoids allocating a new code for a closely related condition.

**`Language` serialization.** `kenn_model::Language` derives `Serialize` with `#[serde(rename_all = "lowercase")]`, so `Language::Python` → `"python"`, `Language::TypeScript` → `"typescript"`, `Language::Csharp` → `"csharp"`, `Language::Rust` → `"rust"`. The `[language.*]` config keys use the same lowercase form, so `enabled_languages` round-trips cleanly between config and JSON payloads.

**Example `configured-but-empty` message** (illustrative, not mandated by the spec): `"Python is enabled in kenn.toml but the snapshot is empty. Check kenn status / report.json for indexer failures, or confirm the workspace contains .py files outside excluded directories."` The spec allows the implementation to either include a most-common-cause hint ("no `.py` files were found") or fall back to "reason unclear" — both compliant.

### kenn does not choose the Python runtime

The user picks the launcher tokens (`["scip-python"]`, `["bunx", "@sourcegraph/scip-python"]`, `["npx", "--yes", "@sourcegraph/scip-python"]`, `["uvx", "scip-python"]`, etc.) and kenn honors them verbatim — no auto-detection, no runtime preference, no bun-vs-npm fallback. This is a deliberate design choice, not an oversight: runtime selection is operator policy (Python users may use pip, JS-leaning developers may use bun, CI may use a pinned npx). Encoding any kenn-side default beyond `["scip-python"]` (a plain PATH lookup) would push that policy into the tool. The Tier-2 probe checks only `command[0]`, matching the same operator-policy stance.

### Tier-2 spec reconciled, not deferred

The pre-existing `scip-indexer` Tier-2 requirement said a missing CLI "MUST emit `Tier 2 unavailable` and continue with other languages." That contradicted both the actual `preflight()` in `crates/kenn-indexer/src/pipeline.rs` (hard-fails with `PipelineError::MissingCli`) and the `indexing-orchestrator` "fail the run in the prepare phase" requirement. The `MODIFIED Requirements` block on *Tier-2 availability detection* in this change replaces the legacy "continue" text with the actual prepare-phase-fail behavior — eliminating the conflict rather than deferring it.

### Indexable-unit discovery reconciled for Python

The pre-existing *Indexable-unit discovery* requirement listed "package roots for Python" as the unit kind. That doesn't match how scip-python actually works (it loads the project graph regardless of scoping, so emitting one unit at the workspace root is both correct and minimum-work). The `MODIFIED Requirements` block replaces "package roots for Python" with "workspace root for Python" and inlines the Python-specific skip rules (`__pycache__/`, `.venv/`, `venv/`, `.kenn/`; `node_modules/` already covered by the default explicit-exclude globs).

### Empty-`command` rejection lives in `Config::load`, not serde

kenn-config today uses plain `serde(deny_unknown_fields)` derives with no `try_from`/`deserialize_with` validators. Implementing the "reject `command = []`" requirement via a per-field deserializer would expand the serde surface unnecessarily. The cleaner shape is a post-load walk in `Config::load` (or a `Config::validate()` helper called from `load`) that visits each `[language.*]` block, checks `command.is_empty()`, and returns a clear error naming the offending language. This keeps serde uniform and puts the (small) validation surface in one obvious place.

### Pre-existing `scip-dotnet` vs `kenn-dotnet` naming inconsistency

The current `scip-indexer` spec has lingering "scip-dotnet" references outside the requirements this change modifies (e.g., the "scip-dotnet index <sln>" mention in *Per-language indexer dispatch* and a few scenarios elsewhere). kenn actually ships the C# driver as `kenn-dotnet` (a JSONL indexer, not a scip-* binary). The `MODIFIED` blocks in this change use the actual driver names; the residual legacy references are out of scope for a Python-support change and should be cleaned up in a focused naming pass.

## Risks / Trade-offs

- **Breaking `kenn.toml` schema** → Mitigation: single known user, driving the change; commit message and proposal call it out; `serde(deny_unknown_fields)` will fail loudly with the old field names, which is the desired UX (no silent ignore).
- **`Vec<String>` launcher gives users enough rope to break themselves** (typos, wrong package name) → Mitigation: the phase-1 preflight calls `command()` to verify the executable is on PATH and fails the run in the prepare phase (per the modified Tier-2 requirement) with a clear `<language>: required command \`<token>\` not found on PATH` message, before any store write. The user sees the bad token early; nothing is half-committed.
- **scip-python is slow on large repos** (it loads Pyright and re-analyses everything; no incremental mode) → Out of scope for this change; users with large Python repos can disable Python in `kenn.toml`. Performance knobs can land later if needed.
- **scip-python emits `Document.Language = ""`** (unlike scip-typescript / rust-analyzer) → Already handled: `transform.rs` falls back to path extension (`.py`/`.pyi` → `Language::Python`). Verified in existing tests.

## Migration Plan

One-shot edit to `kenn.toml` for the lone user:

```toml
# Before
[language.csharp]
kenn_dotnet_path = "/path/to/kenn-dotnet"  # optional

[language.rust]
enabled = true
rust_analyzer_path = "/path/to/rust-analyzer"  # optional
```

```toml
# After
[language.csharp]
enabled = true           # was implicit-true
command = ["kenn-dotnet"]  # default; omit if happy with PATH

[language.rust]
enabled = true
command = ["rust-analyzer"]  # default

[language.python]
enabled = true
command = ["bunx", "@sourcegraph/scip-python"]
```

No data migration. Old snapshots remain readable (record shape unchanged); next `kenn index` produces a new snapshot.

## Open Questions

None — the design tracks the user's two explicit asks ("all lang configs should work the same", "C# should also be enabled as others") plus the binary-resolution question already answered ("use config, some like bun, others npm").
