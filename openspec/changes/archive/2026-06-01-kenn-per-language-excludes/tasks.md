> **Dependency**: supersedes the exclude-handling parts of `kenn-default-excludes-cleanup` and `kenn-python-scoping`. Both archives stay in history; the main spec gets the per-language shape on archive.

## 1. Config — remove cross-language, add per-language

- [x] 1.1 In `crates/kenn-config/src/lib.rs`, REMOVE `ExcludeConfig`, the `[exclude]` section on `Config`, `WORKSPACE_ALWAYS_EXCLUDED`, and `apply_defaults`'s exclude logic (the function may go away entirely if no other defaults need late materialization).
- [x] 1.2 REMOVE `PythonConfig.exclude_documents` (subsumed by `PythonConfig.excludes`).
- [x] 1.3 Add `WorkspaceConfig.excludes: Vec<String>` with `#[serde(default)]` (default empty Vec).
- [x] 1.4 Add `RustConfig.excludes`, `TypescriptConfig.excludes`, `CsharpConfig.excludes`, `PythonConfig.excludes`: `Vec<String>` with `#[serde(default = "default_<lang>_excludes")]` returning the language's `DEFAULT_EXCLUDES` constant materialized as `Vec<String>`.
- [x] 1.5 Keep / fix the per-language `DEFAULT_EXCLUDES` constants on each `*Config`. The Python set adds `__pycache__/**` etc. from kenn-default-excludes-cleanup (already drafted in WIP).
- [x] 1.6 `Default for *Config` impls populate `excludes` from the constant so `Config::default()` matches `Config::from_toml("")`.
- [x] 1.7 Tests:
  - `default_excludes_constants_are_disjoint` (carried over from WIP).
  - `python_excludes_field_defaults_to_python_constant` — no-section TOML, Python enabled, `c.language.python.excludes` equals `PythonConfig::DEFAULT_EXCLUDES`.
  - `user_set_python_excludes_replaces_default` — `[language.python] excludes = ["worked/**"]` resolves to exactly `["worked/**"]`.
  - `explicit_empty_excludes_opts_out` — `[language.python] excludes = []` resolves to empty.
  - `legacy_exclude_section_errors` — `[exclude] globs = []` → `ConfigError::Toml` due to `deny_unknown_fields`.
  - `legacy_exclude_documents_errors` — `[language.python] exclude_documents = []` → `ConfigError::Toml`.

## 2. `Workspace` — per-language storage + uniform API

- [x] 2.1 In `crates/kenn-indexer/src/canonicalize.rs::Workspace`, rename `excludes: GlobSet` to `workspace_excludes: GlobSet` and add per-language `rust_excludes`, `typescript_excludes`, `csharp_excludes`, `python_excludes: GlobSet`. Drop the old Python-specific `python_excludes` from `kenn-python-scoping` (the name and storage are repurposed).
- [x] 2.2 In `Workspace::new`, build `workspace_excludes` from `user_globs` + the hardcoded `.git/**` and `**/.git/**`. NO language defaults here.
- [x] 2.3 Add builder methods:
  - `with_workspace_excludes(self, patterns: &[String]) -> Result<Self, CanonicalizeError>` — overwrite `workspace_excludes` (still always includes `.git/**`).
  - `with_language_excludes(self, language: Language, patterns: &[String]) -> Result<Self, CanonicalizeError>` — set the per-language GlobSet.
  Replace the old `with_python_exclude_documents` with the generic `with_language_excludes`.
- [x] 2.4 Add `pub fn is_excluded(&self, language: Language, relative_path: &str) -> bool` that normalizes separators to `/` and matches against the language's GlobSet. Replaces `is_python_excluded_document` and `is_walk_excluded`.
- [x] 2.5 `Workspace::canonicalize` continues to consult `workspace_excludes` only (path normalization already there at canonicalize.rs:302).
- [x] 2.6 Tests:
  - `is_excluded_python_matches_workspace_relative`.
  - `is_excluded_normalizes_windows_separators` — construct a `PathBuf` with `\\` and assert match through normalization.
  - `is_excluded_does_not_consult_other_languages` — Python excludes set; calling with `Language::Csharp` returns false.
  - `canonicalize_rejects_workspace_excluded_path` — `.git/foo.py` rejected as `Excluded` regardless of language.
  - `canonicalize_does_not_reject_per_language_path` — `__pycache__/foo.cs` reaches canonicalize and returns `Ok` (Python's excludes do not gate C#).

## 3. Discovery walker — workspace-aware pruning

- [x] 3.1 Extend `walk_skipping` in `crates/kenn-indexer/src/driver/mod.rs` with a `dir_skip: impl Fn(&Path) -> bool` parameter (existing `walk` callers pass a no-op closure).
- [x] 3.2 Add `walk_for_language<'a>(workspace: &'a Workspace, language: Language) -> impl Iterator<Item = io::Result<PathBuf>> + 'a` that calls `walk_skipping` with the standard skip leaves plus a closure that returns `workspace.workspace_excludes.is_match(rel) || workspace.is_excluded(language, rel)` on directory paths.
- [x] 3.3 In `ScipPython::discover_units`, drop the per-file `is_walk_excluded` filter; switch to `walk_for_language(workspace, Language::Python)`.
- [x] 3.4 Migrate `RustAnalyzer::discover_units`, `KennDotnet::resolve_projects` to `walk_for_language` (each language self-prunes its own conventions: Rust's `target/`, .NET's `bin/obj/`). If any driver has constraints that don't fit the helper shape, document as a follow-up.
- [x] 3.5 Tests:
  - `walk_for_language_does_not_recurse_into_excluded_dir` — populated `.venv/` with `.py` files; `walk_for_language(ws, Python)` MUST NOT yield from `.venv/` and MUST NOT call `read_dir` on it.
  - `walk_for_language_csharp_does_not_prune_python_excludes` — same workspace; calling `walk_for_language(ws, Csharp)` walks into `.venv/` normally.

## 4. Transform — per-language ingest-time filter

- [x] 4.1 In `crates/kenn-indexer/src/transform.rs::transform_document`, replace the Python-specific `workspace.is_python_excluded_document` check with `workspace.is_excluded(language, &doc.relative_path)`. Now fires for every language.
- [x] 4.2 Update existing `transform_document_drops_python_doc_matching_exclude` and siblings: they should now use `with_language_excludes(Python, ...)` to attach the set; semantics unchanged for Python.
- [x] 4.3 Add `transform_document_drops_csharp_doc_matching_csharp_exclude` — sanity check that the generalization works for a second language.

## 5. Wire-up

- [x] 5.1 `crates/kenn-cli/src/cmd_index.rs::build_driver` and `crates/kenn-indexer/src/workflow.rs::configure_runner`:
  - Replace `Workspace::new(root, &config.exclude.globs)` with `Workspace::new(root, &config.workspace.excludes)` (or rename the new field accessor accordingly).
  - Drop the `with_python_exclude_documents(&config.language.python.exclude_documents)` call.
  - For each enabled language, attach its `excludes` via `with_language_excludes(Language::X, &config.language.X.excludes)`.
- [x] 5.2 Drop `ScipPython.targets` doc-comment references to `exclude_documents` (the field is gone; mentions in docs should point at `[language.python].excludes`).

## 6. Starter `kenn.toml`

- [x] 6.1 Delete the `[exclude]` block entirely.
- [x] 6.2 In each `[language.X]` block, add a commented-out `# excludes = [...]` line showing that language's default set verbatim. Same shape across all four languages so users see the pattern.
- [x] 6.3 Add a short comment under `[workspace]`: "`excludes = []` for additional workspace-wide skip patterns. `.git/**` and linked git worktrees are always skipped; this list is appended."

## 7. Verification

- [x] 7.1 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 7.2 `cargo test -p kenn-config -p kenn-indexer -p kenn-cli -p kenn-mcp` green.
- [x] 7.3 `just crap-ci` no regression.
- [x] 7.4 Re-index `tmp/compose-fixture` (Python-only, build/ present): doc count = 11 with default Python excludes (build/ filtered); doc count = 13 with `[language.python] excludes = []` (build/ included). Matches the kenn-default-excludes-cleanup verification.
- [x] 7.5 Re-index `tmp/graphify` with `[language.python] excludes = ["__pycache__/**", "**/__pycache__/**", ".venv/**", "**/.venv/**", "worked/**"]` (defaults + the worked/ filter): document count MUST match the kenn-python-scoping §5.4 result (77 documents).
- [x] 7.6 `cargo fmt --all`.
