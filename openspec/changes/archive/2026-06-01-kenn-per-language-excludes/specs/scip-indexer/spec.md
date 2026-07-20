# scip-indexer

## MODIFIED Requirements

### Requirement: Workspace exclude-glob fallback at config load

The indexer SHALL replace the cleanup-era global `[exclude].globs` fallback with per-language excludes scoped to each language's pipeline. The `[exclude]` section MUST be removed from the config schema.

The exclude model SHALL be:

- `[workspace].excludes: Vec<String>` (optional, default `[]`) — workspace-wide patterns. The runtime SHALL hardcode `.git/**` and `**/.git/**` plus the auto-discovered linked git worktrees from `Workspace::excluded_dirs()`. `[workspace].excludes` SHALL be the ONLY exclude set consulted cross-language; `Workspace::canonicalize` consults it and gates documents from every driver.
- `[language.X].excludes: Vec<String>` for every supported language (`rust`, `typescript`, `csharp`, `python`). Each field SHALL default to that language's conventional set via `*Config::DEFAULT_EXCLUDES` — the serde default produces a fresh `Vec<String>` from the constant. User-supplied values MUST REPLACE the default fully; no implicit merge.

Per-language `excludes` SHALL be consulted EXCLUSIVELY by that language's driver (discovery walker) and that language's transform (per-document filter). They MUST NOT gate documents emitted by other languages.

The previous global `[exclude].globs` and Python's `[language.python].exclude_documents` MUST be removed. TOML files containing either field MUST cause config load to fail under `deny_unknown_fields`.

#### Scenario: Rust-only workspace, no `[exclude]` section

- **WHEN** only `[language.rust].enabled = true` and no `[exclude]` or `[workspace].excludes`
- **THEN** the resolved Rust exclude set MUST be `RustConfig::DEFAULT_EXCLUDES` (`target/**`, `**/target/**`)
- **AND** the workspace exclude set MUST be exactly `.git/**`, `**/.git/**` plus auto-discovered worktrees
- **AND** Python's, TypeScript's, C#'s excludes MUST NOT influence canonicalize or any driver

#### Scenario: Python-only workspace, no `[exclude]` section

- **WHEN** only `[language.python].enabled = true` and no `[exclude]` or `[workspace].excludes`
- **THEN** the resolved Python exclude set MUST be `PythonConfig::DEFAULT_EXCLUDES`
- **AND** the workspace exclude set MUST contain ONLY `.git/**` and `**/.git/**` (plus worktrees)
- **AND** a `target/foo.py` Document from scip-python MUST be ingested normally (Rust's `target/**` does not influence Python's pipeline)

#### Scenario: User-supplied per-language excludes replace defaults

- **WHEN** the user configures `[language.python] excludes = ["worked/**"]`
- **THEN** the resolved Python exclude set MUST be exactly `["worked/**"]`
- **AND** `__pycache__/foo.py` MUST be ingested (the default was replaced; user did not list `__pycache__/**`)

#### Scenario: Explicit empty list opts out

- **WHEN** the user configures `[language.python] excludes = []`
- **THEN** the resolved Python exclude set MUST be empty
- **AND** the Python driver and transform MUST NOT skip any path on Python's behalf

#### Scenario: Workspace excludes gate cross-language

- **WHEN** the user configures `[workspace] excludes = ["sensitive/**"]` AND BOTH Python and C# are enabled
- **THEN** a Document with `relative_path = "sensitive/foo.py"` MUST be rejected by canonicalize for Python
- **AND** a Document with `relative_path = "sensitive/foo.cs"` MUST be rejected by canonicalize for C#

#### Scenario: Per-language exclude does NOT leak across languages

- **WHEN** `[language.python] excludes = ["__pycache__/**"]` (the default) AND `[language.csharp].enabled = true`
- **AND** an out-of-band SCIP file from kenn-dotnet contains a Document with `relative_path = "__pycache__/foo.cs"`
- **THEN** the C# transform MUST ingest that Document normally
- **AND** the path MUST NOT be rejected by canonicalize

#### Scenario: Legacy `[exclude]` section is a hard error

- **WHEN** the user's TOML contains `[exclude] globs = ["foo/**"]`
- **THEN** config load MUST fail with a `deny_unknown_fields` error naming the `[exclude]` section
- **AND** no fallback substitution MUST occur

#### Scenario: Legacy `exclude_documents` field is a hard error

- **WHEN** the user's TOML contains `[language.python] exclude_documents = ["worked/**"]`
- **THEN** config load MUST fail with a `deny_unknown_fields` error naming the field

## ADDED Requirements

### Requirement: Per-language `is_excluded` API on `Workspace`

`Workspace` SHALL expose `is_excluded(language: Language, relative_path: &str) -> bool`. The function:

1. Normalizes the relative path to forward-slash separators (mirroring `canonicalize`'s normalization).
2. Matches the normalized path against that language's `GlobSet` only.
3. Returns `true` on match, `false` otherwise.

The workspace-level exclude check (cross-language, performed by `canonicalize`) is NOT consulted by `is_excluded` — canonicalize is the only path through which workspace excludes apply, and `is_excluded` is for callers downstream of canonicalize.

#### Scenario: Per-language match on macOS / Linux

- **WHEN** Python's exclude set contains `__pycache__/**` AND the relative path is `__pycache__/foo.py`
- **THEN** `workspace.is_excluded(Language::Python, "__pycache__/foo.py")` MUST return `true`

#### Scenario: Windows-style separator does not break match

- **WHEN** Python's exclude set contains `__pycache__/**` AND the relative path representation uses `\\` (constructed via `Path::new("__pycache__\\\\foo.py")` on Windows)
- **THEN** the normalized form MUST match the pattern; `is_excluded` MUST return `true`

#### Scenario: Other languages are not consulted

- **WHEN** Python's exclude set contains `__pycache__/**` AND C#'s exclude set is empty
- **THEN** `workspace.is_excluded(Language::Csharp, "__pycache__/foo.cs")` MUST return `false`

### Requirement: Workspace-aware discovery walker prunes excluded directories per language

SCIP drivers' discovery walkers SHALL use a workspace-aware helper `walk_for_language(workspace, language)` that prunes directory recursion when EITHER the directory matches `workspace.workspace_excludes` OR `workspace.is_excluded(language, dir)`. A populated `.venv/` (for Python) or `target/` (for Rust) MUST NOT be descended into; the walker MUST NOT call `read_dir` on such directories.

This requirement subsumes the pruning behavior introduced by `kenn-default-excludes-cleanup` and corrects the per-file post-filter perf regression noted in the review of that change.

#### Scenario: Walker prunes a language-specific excluded directory

- **WHEN** `walk_for_language(workspace, Language::Python)` is called on a workspace where Python's excludes contain `.venv/**` AND `.venv/lib/site-packages/foo.py` exists
- **THEN** the iterator MUST NOT yield `.venv/lib/site-packages/foo.py`
- **AND** the implementation MUST NOT call `read_dir` on `.venv/`

#### Scenario: Walker for a different language does not prune

- **WHEN** `walk_for_language(workspace, Language::Csharp)` is called on the same workspace with Python excludes `.venv/**` AND C# excludes empty
- **THEN** the iterator MUST yield files under `.venv/` (assuming none match C#'s extension filter — the prune does not fire because Python's excludes don't influence the C# walker)

### Requirement: Per-language transform consults its own exclude set at ingest

The SCIP→record transform for language `L` SHALL consult `workspace.is_excluded(L, &document.relative_path)` before emitting any record from a Document. When the check returns `true`, the transform MUST drop the document: no `SymbolRecord`, no `DefRecord`, no occurrence-derived edge. Cross-document edges referencing symbols defined inside a dropped document continue to emit via the existing `external_symbols` path (`is_external = true` stubs).

The Python-only `is_python_excluded_document` introduced by `kenn-python-scoping` is removed. Its responsibilities are subsumed by `is_excluded(Language::Python, ...)`.

#### Scenario: Python transform drops document matching its excludes

- **WHEN** Python's excludes contain `worked/**` AND scip-python emits a Document with `relative_path = "worked/httpx/raw/transport.py"`
- **THEN** the transform MUST NOT emit any record from that Document

#### Scenario: Non-matching Python document is ingested normally

- **WHEN** Python's excludes contain `worked/**` AND scip-python emits a Document with `relative_path = "graphify/detect.py"`
- **THEN** the transform MUST ingest every record per the existing requirements

#### Scenario: C# transform unaffected by Python's excludes

- **WHEN** Python's excludes contain `__pycache__/**` AND kenn-dotnet emits a Document with `relative_path = "__pycache__/foo.cs"`
- **THEN** the C# transform MUST ingest the Document normally (Python's exclude set is not consulted by the C# transform)
