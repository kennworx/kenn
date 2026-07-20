## Why

The `kenn-default-excludes-cleanup` change moved exclude defaults into kenn-config but bundled them into a global cross-language fallback. That's wrong on two axes:

1. **Patterns are language-specific by origin and ergonomics.** `bin/`, `obj/` are MSBuild output but `src/bin/` is the canonical Rust multi-binary layout; `target/` is Cargo output but harmless to other languages; `__pycache__/`, `.venv/` are Python; `node_modules/` is Node/TS. A global bundle hides legitimate code from non-matching workspaces.
2. **The global gate leaks across languages.** Once a pattern is in the workspace exclude set, `Workspace::canonicalize` rejects it for every driver. A path that looks like `__pycache__/foo.cs` (contrived but legal on disk) gets dropped before the C# driver ever sees it. Language excludes should be scoped to that language's pipeline only.

The `kenn-python-scoping` change added `[language.python].exclude_documents` as a Python-only ingest-time filter, working around the global. With per-language excludes done properly, that workaround collapses into the per-language field.

## What Changes

- **Remove `[exclude]` section entirely.** `[exclude].globs`, the `DEFAULT_EXCLUDE_FALLBACK` constant, and `WORKSPACE_ALWAYS_EXCLUDED` from kenn-default-excludes-cleanup all go away.
- **Remove `[language.python].exclude_documents`** introduced by kenn-python-scoping. Subsumed by the new per-language `excludes`.
- **Add `[workspace].excludes: Vec<String>`** for workspace-wide additional patterns. Defaults to `[]`. `.git/**` is hardcoded into the walker / canonicalize layer (it's a kenn invariant, not a config knob); linked git worktrees are auto-discovered as before. Most users never set this.
- **Add `[language.X].excludes: Vec<String>`** to every language config (`RustConfig`, `TypescriptConfig`, `CsharpConfig`, `PythonConfig`). Each field has `#[serde(default = "default_X_excludes")]` returning that language's conventional set:
  - Rust → `["target/**", "**/target/**"]`
  - TypeScript → `["node_modules/**", "**/node_modules/**"]`
  - C# → `["bin/**", "**/bin/**", "obj/**", "**/obj/**"]`
  - Python → `["__pycache__/**", "**/__pycache__/**", ".venv/**", "**/.venv/**", "venv/**", "**/venv/**", ".tox/**", "**/.tox/**", "dist/**", "**/dist/**", "build/**", "**/build/**", "*.egg-info/**", "**/*.egg-info/**"]`
- **User-set `excludes` REPLACES the default fully.** No implicit merge. Documented opt-out shape: `excludes = []` (empty list opts out completely; any non-empty list replaces). Matches Python's existing `exclude_documents` semantics.
- **`Workspace` storage**: drop the cross-language `excludes: GlobSet`. Add four per-language `GlobSet`s (`rust_excludes`, `typescript_excludes`, `csharp_excludes`, `python_excludes`) plus `workspace_excludes` (built from `[workspace].excludes`). Add `workspace_excludes` to the canonicalize gate (so `.git/**` and any user-set workspace pattern still gates cross-language); per-language excludes consulted only by their own driver / transform.
- **`Workspace::canonicalize`** gates on `workspace_excludes` only (no cross-language language excludes). A `.cs` file under `__pycache__/` still reaches the C# transform.
- **Per-language `is_excluded(language, path)` and `walk_for_language(language)` helpers** on `Workspace`. Each driver's discovery walker calls `walk_for_language(its_language)` which prunes on `workspace_excludes ∪ language_excludes`. Each language's transform calls `is_excluded(language, &doc.relative_path)` to drop matching documents.
- **Drop the Python-specific `python_excludes: GlobSet` field** added by kenn-python-scoping (it stored `exclude_documents`). The new `python_excludes` field is built from the new per-language `[language.python].excludes` instead. Name reused; semantics generalized.
- **Hard migration**: `[exclude]` section and `[language.python].exclude_documents` are removed. `deny_unknown_fields` will fail-fast on any TOML that still uses them. No silent compatibility shim — no real external users yet, and the rename is mechanical.
- **Starter `kenn.toml`** updated: drop the giant commented `[exclude]` block; add an `excludes = [...]` field commented inside each `[language.X]` block showing that language's default.
- **Review-issue fixes from `kenn-default-excludes-cleanup`** that also land here:
  - **Windows separator bug**: `Workspace::is_walk_excluded` (and its successors) normalize backslashes to forward slashes before glob matching.
  - **Walker perf regression**: discovery walkers prune at directory level, not per file.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `scip-indexer`: replace the *Workspace exclude-glob fallback at config load* requirement (added by `kenn-default-excludes-cleanup`) with a per-language exclude model. Remove the cross-language gate from canonicalize; route each language's excludes through that language's driver and transform only.

## Impact

- **Code**: `crates/kenn-config/src/lib.rs` (drop `ExcludeConfig`/`WORKSPACE_ALWAYS_EXCLUDED`/`DEFAULT_EXCLUDE_FALLBACK`; add `WorkspaceConfig.excludes` and per-language `excludes` field with default fns; rewrite `apply_defaults` or remove it entirely if defaults are sufficient at serde-level), `crates/kenn-indexer/src/canonicalize.rs` (per-language GlobSets on `Workspace`; refactor `is_walk_excluded`/`python_excludes`/`with_python_exclude_documents` into a uniform per-language API; fix Windows separator), `crates/kenn-indexer/src/driver/mod.rs` (new `walk_for_language` helper; deprecate `walk` for driver callers), `crates/kenn-indexer/src/driver/{python,rust,typescript,dotnet}.rs` (adopt per-language helpers), `crates/kenn-indexer/src/transform.rs` (the Python-only `is_python_excluded_document` check becomes the language-keyed `is_excluded(language, ...)` check), `crates/kenn-cli/src/{cmd_index.rs,starter_kenn.toml}` (wire the new fields).
- **Config**: HARD break. Users with `[exclude]` or `[language.python].exclude_documents` get a clear `deny_unknown_fields` error at load with the migration path visible (the starter `kenn.toml` shows the new shape).
- **No store schema change.**

## Dependencies

- **`kenn-default-excludes-cleanup`** and **`kenn-python-scoping`** — both archived. This change supersedes their exclude-handling sections; the affected spec requirements are rewritten here.
