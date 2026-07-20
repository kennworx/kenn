## Why

scip-python's `Indexer` walks every `.py`/`.pyi` file under the workspace root through Pyright's `program.indexWorkspace`, then emits one `scip.Document` per project source file via `TreeVisitor`. Three needs sit on top of that — two scoping, one test-detection:

1. **Subdirectory scope** — monorepos with multiple Python sub-packages want to index `src/api/` independently of `src/worker/`, instead of one whole-workspace pass.
2. **Workspace excludes that kenn-side discovery already supports** — kenn's existing `[exclude].globs` setting filters out paths during unit discovery, but scip-python is run as a black box and walks every file under `cwd` regardless. Concretely on graphify: a `worked/httpx/raw/` fixture directory ships ~14 files of vendored httpx code; scip-python emits them as project Documents (verified — they get the `graphify` package name because their path starts with cwd), and they show up as god-nodes in the aggregate graph (`Auth`, `BaseTransport`, `HTTPTransport`, etc.).
3. **Test marking** — scip-python never sets the SCIP `SymbolRole.Test` (32) bit (verified on graphify: every occurrence has role 1 (Definition) or 8 (ReadAccess); no 32, no combinations). Kenn's existing `[tests].paths` glob list handles file-level test marking but its default is empty, and the `is_test_descriptor` heuristic returns `false` for Python. Result on graphify (which doesn't configure `[tests].paths`): REPORT.md shows `37 live, 0 test, 5 external` despite 50 documents under `tests/`. Python users need a sensible built-in heuristic so test code is correctly tagged without per-workspace boilerplate.

**Verified against the actual graphify and django SCIP outputs:** every `Document.relative_path` is workspace-relative; zero documents have `..` or absolute paths. The dep-leak in graphify's REPORT.md is **entirely** in-workspace fixture/test code, not transitively-imported deps from site-packages. So the right fix is a **workspace-relative glob filter at ingest**, not a document-path predicate.

C# already addresses the first part of this via `[language.csharp].projects` — a workspace-relative list that overrides auto-discovery and is forwarded to one batched `kenn-dotnet` invocation. Python needs an analog (with the documented cost caveat: scip-python can't batch — each target is a fresh Pyright analysis), plus the glob filter.

## What Changes

- Add `[language.python].targets: Vec<String>` (default `[]`). Workspace-relative subdirectory paths to scope scip-python to.
  - `[]` → today's behaviour (one invocation, no `--target-only`, whole workspace).
  - one entry → one invocation with `--target-only <path>`.
  - N entries → N separate scip-python invocations (each pays its own Pyright analysis cost; kenn merges the resulting `.scip` outputs through the existing per-unit ingest loop).
- Add `[language.python].exclude_documents: Vec<String>` (default `[]`). Workspace-relative glob patterns; any `Document` whose `relative_path` matches at least one pattern is dropped at SCIP→record ingest. Matches the actual graphify dep-leak case (`exclude_documents = ["worked/**"]`). This is independent of and composes with `targets`.
- Extend `is_test_descriptor` (in `crates/kenn-indexer/src/transform.rs`) with Python heuristics: a Python public_id whose dotted segments contain any of `tests` / `test` / `__tests__`, or starts with `test_`, or ends with `_test`, or whose leaf matches `conftest` / `Test*` / `*Test` / `*TestCase` SHALL mark its `SymbolRecord.test = true`. Runs only when the file's `[tests].paths` glob didn't already mark the document (preserving the existing per-workspace override). No new config; this is a baseline that "just works."
- Update the `ScipPython` driver to emit one unit per configured target and forward `--target-only`.
- Update the SCIP→record transform to consult the exclude list before emitting any record from a Document.
- Update the starter `kenn.toml` to mention both knobs.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `scip-indexer`: extend the Python indexer-dispatch requirement with the `--target-only` invocation contract for non-empty `targets`; extend the Python unit-discovery requirement to emit one unit per configured target (or one workspace-root unit when `targets` is empty); add two new requirements — glob-based document filtering at Python ingest, and Python test-marking heuristics applied at descriptor level.

## Dependencies

- **`kenn-python-support`** — this change modifies requirements added there (`Python indexer dispatch via launcher command`, `Python unit discovery`). It depends on `kenn-python-support` being applied and archived first; until then, the MODIFIED blocks target requirements that live in another in-flight change rather than in `openspec/specs/`.

## Impact

- **Code**: `crates/kenn-config/src/lib.rs` (`PythonConfig` gains `targets`, `exclude_documents`); `crates/kenn-indexer/src/driver/python.rs` (multi-unit discovery, `--target-only` arg forwarding, per-unit output slug); `crates/kenn-indexer/src/transform.rs` (consult glob list, drop matching documents before record emission); `crates/kenn-cli/src/cmd_index.rs` + `crates/kenn-indexer/src/workflow.rs` (forward the new fields); `crates/kenn-cli/src/starter_kenn.toml` (one commented example for each).
- **Config**: additive (no breaking changes for users who don't set either knob).
- **Performance**: N targets = N × Pyright analysis cost (no batching — scip-python can't share Pyright state across invocations). Documented in `targets`'s doc-comment.
- **Output filename change**: per-unit slug becomes `python-<idx>.scip` under the active run dir (`.kenn/local/runs/<ts>/`); no user-visible artefact moves.
- **No spec-level impact** on `mcp-server`, `code-intel-data-model`, or `indexing-orchestrator`. The new ingest-time filter is behavior-preserving for users who don't configure it.
