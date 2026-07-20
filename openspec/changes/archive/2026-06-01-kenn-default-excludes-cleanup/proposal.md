## Why

`crates/kenn-indexer/src/canonicalize.rs::DEFAULT_EXCLUDES` is a hardcoded list dressed up as universal, but every entry is in fact language-specific:

```rust
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules/**", "**/node_modules/**",   // Node / TypeScript
    "bin/**",          "**/bin/**",            // .NET MSBuild
    "obj/**",          "**/obj/**",            // .NET MSBuild
    "target/**",       "**/target/**",         // Rust / Cargo
];
```

Three concrete problems:

1. **Pre-Python history**: kenn supported only C# / TS / Rust when these defaults were added. They got bundled into one cross-language list because there was no per-language story yet. Python (added by `kenn-python-support`) has its own conventional excludes — `__pycache__/`, `.venv/`, `venv/`, `.tox/`, `dist/`, `build/`, `*.egg-info/` — that DON'T appear in `DEFAULT_EXCLUDES`. The current `ScipPython` driver compensates with a local `skip_leaves` argument to `walk_skipping`, but that only filters discovery; canonicalize-time `Excluded` rejection still doesn't fire for those Python paths.

2. **Inconsistent layering**: workspace-walking exclusions live in two places — `DEFAULT_EXCLUDES` (consulted by `Workspace.excludes` at canonicalize-time) AND `walk_skipping`'s `skip_leaves` argument (consulted at discovery-walk time). A `__pycache__/foo.py` is filtered by the Python driver's discovery walk but `Workspace::canonicalize("file://…/__pycache__/foo.py")` returns Ok — opposite policy from `node_modules/`.

3. **Implicit > explicit**: users have no way to see what's being excluded by default unless they grep the source. A path that "mysteriously" doesn't show up in the index is hard to debug.

## What Changes

- **Move the defaults into `starter_kenn.toml`'s `[exclude].globs`** (commented + uncommented mix), and drop `DEFAULT_EXCLUDES` from `crates/kenn-indexer/src/canonicalize.rs`. The Workspace builder consults only `user_globs` after the change.
- Add the missing Python defaults (`__pycache__/**`, `.venv/**`, `venv/**`, `.tox/**`, `dist/**`, `build/**`, `*.egg-info/**`) to the starter file.
- Migrate `ScipPython::discover_units`'s ad-hoc `skip_leaves` (`__pycache__`, `.venv`, `venv`, `node_modules`, `.kenn`) into the canonical exclude list so canonicalize-time and discovery-time apply the same policy.
- **Backwards-compat for existing kenn.toml files**: at config-load time (`kenn_config`), if `[exclude].globs` is empty or omitted, layer in a hand-curated fallback (the new union) and `tracing::warn` once that the user is relying on built-in defaults that may shift; users get a clear path to opt out by setting `globs = []` explicitly. This preserves "do something sensible on first install" without hiding the policy in compiled code.
- **No spec changes elsewhere**: `[language.python].exclude_documents` from `kenn-python-scoping` remains the Python-only ingest-time filter. This proposal only touches discovery-time + canonicalize-time exclusion.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `scip-indexer`: amend the workspace-walking + canonicalize-time exclude requirement to source patterns from configuration (`[exclude].globs` plus the documented fallback), not from a hardcoded `DEFAULT_EXCLUDES` constant. The default exclude set as published in the starter `kenn.toml` SHALL include the Python conventional set (`__pycache__/`, `.venv/`, etc.) alongside the existing Node/.NET/Rust entries.

## Impact

- **Code**: `crates/kenn-indexer/src/canonicalize.rs` (drop `DEFAULT_EXCLUDES`; layer the fallback at config-load via `kenn_config` instead), `crates/kenn-indexer/src/driver/python.rs` (drop the ad-hoc `skip_leaves` once the canonical list covers it), `crates/kenn-config/src/lib.rs` (apply the fallback when `[exclude].globs` is empty + emit one-shot warning), `crates/kenn-cli/src/starter_kenn.toml` (uncomment the defaults, add the Python entries).
- **Config**: existing workspaces with NO `[exclude]` section continue to work — fallback fires. Workspaces with `[exclude].globs = ["custom/**"]` get ONLY their custom list — the fallback no longer silently merges in, which is a small behaviour change but correct (explicit > implicit). Migration note for affected users will go in the change's `design.md`.
- **No spec-level impact** on `mcp-server`, `code-intel-data-model`, or `indexing-orchestrator`. Per-language scope filters (`[language.python].exclude_documents`, `[language.csharp].projects`, `[language.python].targets`) are unaffected.
- **No store schema change.**

## Dependencies

- **`kenn-python-scoping`** — the migration of `ScipPython`'s `skip_leaves` depends on `kenn-python-scoping` being applied and archived first (this proposal modifies `discover_units` that the scoping change reshapes around `targets`).
