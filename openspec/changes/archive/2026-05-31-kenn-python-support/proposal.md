## Why

`kenn index` produces an empty snapshot on Python-only workspaces today (graphify: 91 `.py` files → `documents=0 symbols=0`), even though every downstream piece is already wired: `PythonTransformer` maps `scip-python` symbols into `py:` public IDs, the SCIP→record transform recognises `.py`/`.pyi`, and scip-python itself produces a well-formed 4 MB index against graphify. The missing piece is a driver that spawns scip-python and feeds the output into the orchestrator. At the same time the four language config blocks have drifted apart (C# is enabled-by-default and uses `kenn_dotnet_path`; rust/typescript are opt-in and use their own `*_path` fields), so adding Python is a good moment to align them.

## What Changes

- Add a `ScipPython` SCIP driver that walks the workspace for `.py` files, spawns `scip-python index --cwd <ws> --output <out>`, and returns the produced `.scip` for the existing transform pipeline to ingest.
- Add `[language.python]` to the kenn config (`enabled`, `command`, `project_name`, `project_version`).
- **BREAKING**: Unify the four language config blocks on a single shape — `enabled: bool` (default `false` for all) + `command: Vec<String>` (launcher tokens) — replacing the four bespoke `*_path: Option<PathBuf>` fields.
- **BREAKING**: `[language.csharp].enabled` flips from `true` → `false` so C# behaves like the others (explicit opt-in).
- Wire the new driver into `cmd_index::build_driver` and `workflow.rs` alongside the existing three.

## Capabilities

### New Capabilities

(none — Python is already named in the existing `scip-indexer` capability's per-language dispatch rule; this change adds implementation and scenarios but no new capability)

### Modified Capabilities

- `scip-indexer`: add scenarios proving Python unit discovery, scip-python dispatch, and the uniform driver-config shape (launcher command vector); flip the default-enablement scenario to require opt-in for every language including C#.
- `mcp-server`: add scenarios proving MCP read tools surface a structured config-driven error against empty snapshots — distinguishing `config-disabled` (every `[language.*].enabled = false`) from `configured-but-empty` (enabled language found nothing) — so a "no languages enabled" state is diagnosable from the MCP client without inspecting `kenn.toml` separately.

## Impact

- **Code**: new `crates/kenn-indexer/src/driver/python.rs`; field renames in `crates/kenn-indexer/src/driver/{dotnet,rust,typescript}.rs` (`binary_path: Option<PathBuf>` → `command: Vec<String>`); `LanguageConfig` updates in `crates/kenn-config/src/lib.rs`; wire-up at `crates/kenn-cli/src/cmd_index.rs::build_driver` and `crates/kenn-indexer/src/workflow.rs`.
- **Config**: `kenn.toml` schema changes — four `*_path` fields disappear and become `command = [...]` arrays; C# users must add `enabled = true` explicitly. No back-compat shim (single known user, driving the change).
- **External dependency**: scip-python (the user supplies a launcher — `["scip-python"]`, `["bunx", "@sourcegraph/scip-python"]`, or `["npx", "--yes", "@sourcegraph/scip-python"]`).
- **No spec-level impact** on `indexing-orchestrator`, `code-intel-data-model`, or any data-store specs — the new driver fits the existing `ScipDriver` trait and produces records the existing transform already handles.
