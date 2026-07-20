## Context

After `kenn-default-excludes-cleanup` and `kenn-python-scoping`, the exclude story is split across three layers:

- `kenn_config::DEFAULT_EXCLUDE_FALLBACK` — cross-language bundle applied by `Config::apply_defaults` when `[exclude].globs` is empty.
- `Workspace.excludes: GlobSet` — cross-language gate consulted by `Workspace::canonicalize` and `Workspace::is_walk_excluded`.
- `Workspace.python_excludes: GlobSet` — Python-only ingest-time filter (from `[language.python].exclude_documents`).

Two of these were retrofits around the third: `exclude_documents` exists *because* `[exclude].globs` was cross-language and the user couldn't say "drop this only for Python." The fallback bundle exists because the original design treated `bin/`/`obj/`/`target/`/`node_modules/` as workspace properties when they're language properties.

The cleaner shape is one uniform per-language `excludes` field and no cross-language exclude gate. Each language owns what it considers unindexable; what's "workspace-wide" is genuinely workspace stuff only (`.git/`, linked git worktrees, optional user overrides).

## Goals / Non-Goals

**Goals:**
- One config field per language, one workspace-level field, single name `excludes` everywhere.
- Per-language excludes scoped to that language's pipeline only (driver + transform). Never gate other languages on Python's `__pycache__/**`.
- Walk-time AND ingest-time both consult the same per-language set (no duplicate fields).
- Defaults travel as `*Config::DEFAULT_EXCLUDES` consts; serde uses them as the field default via `#[serde(default = "default_X_excludes")]`.
- Remove `[exclude]` and `[language.python].exclude_documents` cleanly — hard error on legacy TOML, no compat shim.
- Fix the Windows separator bug and walker perf regression while we're touching these code paths.

**Non-Goals:**
- A merge-vs-replace knob for user `excludes`. User-set replaces fully; explicit > implicit.
- Per-language `[language.X].exclude_documents` as a sibling field. We're collapsing not multiplying.
- Adding excludes to additional non-language sections (`[ingest]`, `[metrics]`, etc.). Excludes are a workspace + language concept only.

## Decisions

### One field name everywhere: `excludes`

`[workspace].excludes` and `[language.X].excludes`. No `walk_excludes` vs `exclude_documents` vs `globs`. A user looking at the TOML doesn't have to wonder which knob applies to their use case — there's one knob per scope, same name.

### User-set REPLACES default

`[language.python].excludes = ["worked/**"]` resolves to exactly `["worked/**"]`. Python's conventional `__pycache__/**` etc. are NOT merged in. To opt out completely: `excludes = []`. To add to defaults: copy the defaults from `PythonConfig::DEFAULT_EXCLUDES` into your TOML and add your patterns.

**Alternative considered**: implicit merge (`user_list ∪ defaults`). Rejected — hides behavior; users editing the field don't see the full resolved set in the TOML; surprising when removing a default fails to actually remove it. The explicit-replace shape matches `kenn-python-scoping`'s `exclude_documents` and the cleanup-era opt-out idiom.

### Workspace-level excludes for workspace-internal stuff only

`[workspace].excludes` defaults to `[]`. The runtime ALWAYS adds:
- `.git/**` and `**/.git/**` — kenn-internal invariant, never source.
- Linked git worktrees from `Workspace::excluded_dirs()` — already discovered.

`.kenn/**` is NOT here. The derived store (`.kenn/local/runs/<ts>/...`) lives under `derived_root` which the layout layer manages; no SCIP indexer is pointed at `.kenn/`. If a paranoid user wants to add it, they put it in `[workspace].excludes`.

`workspace.excludes` IS consulted cross-language (gates in `Workspace::canonicalize`). The justification: anything in `[workspace].excludes` is genuinely workspace-internal — git metadata, build orchestration files, etc. — and shouldn't reach any indexer. A `.cs` file under `.git/` would be data, not code.

### Per-language excludes consulted ONLY by that language

Two storage GlobSets per supported language (e.g., `python_excludes` on `Workspace`, holding the patterns from `[language.python].excludes`). Two consumers per language:
- The driver's discovery walker (perf: prune `.venv/` from recursion).
- The transform layer at the per-document hook (correctness: drop `scip.Document`s the indexer emitted from those paths).

A third language seeing the same path is unaffected. `__pycache__/foo.cs` (contrived but legal) is rejected by Python's exclude check only when the Python driver / transform looks at it; the C# driver sees the same path normally.

### `Workspace::canonicalize` keeps a cross-language gate ONLY for `workspace.excludes`

Today canonicalize calls `self.excludes.is_match(rel)` and returns `Excluded` on match. The same logic stays, but `self.excludes` now means the workspace-level set (`workspace.excludes` + hardcoded `.git/**`), not the cross-language bundle.

Per-language excludes are NOT checked in canonicalize. They're checked one layer up, in `transform_document`, after canonicalize returns Ok. This keeps the canonicalize contract pure (path translation + workspace-internal filter) and pushes language semantics into the language-aware transform layer where they belong.

### `Workspace` API

New per-language fields and accessors:

```rust
pub struct Workspace {
    workspace_excludes: GlobSet,   // [.git/**, **/.git/**] ∪ [workspace].excludes
    rust_excludes: GlobSet,
    typescript_excludes: GlobSet,
    csharp_excludes: GlobSet,
    python_excludes: GlobSet,
    // existing: root, layout, run_dir, excluded_dirs, tests
}

impl Workspace {
    /// True if a path is excluded for the given language (per-language
    /// patterns; does NOT consult workspace_excludes since canonicalize
    /// already gated on those).
    pub fn is_excluded(&self, language: Language, relative_path: &str) -> bool;
}
```

`is_excluded` does the path-separator normalization once and matches against the right GlobSet by `Language`. Builders:

```rust
impl Workspace {
    pub fn with_workspace_excludes(self, patterns: &[String]) -> Result<Self, CanonicalizeError>;
    pub fn with_language_excludes(self, language: Language, patterns: &[String]) -> Result<Self, ...>;
}
```

Or one wider builder that takes the whole `&Config` and wires everything. Pick whichever has less repetition at the call sites.

### Discovery walker

Replace the existing `walk` family with:

```rust
pub(crate) fn walk_for_language<'a>(
    workspace: &'a Workspace,
    language: Language,
) -> impl Iterator<Item = io::Result<PathBuf>> + 'a
```

That internally calls `walk_skipping` with leaf set `["bin"?, "obj"?, ".git"]` (still useful as cheap pre-check) AND a directory-skip closure that returns `workspace.is_excluded(language, rel) || workspace.workspace_excludes.is_match(rel)`. So a `.venv/` is pruned (Python), a `target/` is pruned (Rust), a `.git/` is pruned (workspace).

Drivers update to `walk_for_language(workspace, Language::Python)` etc.

### Default constants stay on the Config structs

Same shape as the in-progress kenn-per-language-excludes work-in-progress:

```rust
impl PythonConfig {
    pub const DEFAULT_EXCLUDES: &'static [&'static str] = &[...];
}
```

The serde default function calls into the const:

```rust
fn default_python_excludes() -> Vec<String> {
    PythonConfig::DEFAULT_EXCLUDES.iter().map(|s| (*s).to_string()).collect()
}
```

So a user-omitted field gets the constant; a user-set field gets exactly what they wrote.

### Migration: hard error

Users with `[exclude]` or `[language.python].exclude_documents` in their TOML see:

```
kenn: TOML parse error
unknown field `exclude`, expected one of `workspace`, `language`, ...
```

The starter `kenn.toml` shows the new shape inline. Migration is mechanical: rename the section / field, copy patterns. No silent compat — `deny_unknown_fields` does its job.

### Hardcoded `.git/**` in the walker

The walker's `skip_leaves` already includes `.git`. We keep it there as a cheap leaf-name short-circuit (avoids the GlobSet match for the most common case). The `workspace_excludes` set ALSO contains `.git/**` so canonicalize agrees. Some duplication; the walker's check is per-directory cheap, the GlobSet match is per-path.

### Windows path separator

`Workspace::is_excluded` and the walker's directory-skip closure both normalize `MAIN_SEPARATOR` to `/` before matching. Same normalization as `canonicalize`. Add a unit test that exercises the normalization explicitly.

### Walker perf

`walk_skipping` extended to take a `dir_skip: impl Fn(&Path) -> bool` parameter (default closure returns `false` for the public `walk` callers). `walk_for_language` passes a real closure that consults `workspace_excludes` and `language_excludes` at directory-recursion time, pruning before `read_dir`.

## Risks / Trade-offs

- **Hard break on legacy `[exclude]` / `exclude_documents`** — Mitigated by no real external users yet. Migration is mechanical.
- **User-set `excludes = ["custom/**"]` loses language defaults** — Documented: copy defaults if you want them. Alternative was hidden-merge, rejected for transparency. Real users will tell us if this hurts.
- **Per-language storage on `Workspace`** — More fields. Pays for itself with the per-language scoping correctness; the alternative `HashMap<Language, GlobSet>` is uglier at the call sites without an offsetting benefit.
- **`.kenn/**` no longer always-excluded** — kenn-default-excludes-cleanup had it; this change drops it. Rationale: the layout layer owns `.kenn/`; no SCIP indexer is pointed at it. If a user enables Python and somehow ends up with `.kenn/foo.py`, Python's `walk_for_language` won't auto-skip it, but no indexer would emit a `.py` from there either. Paranoid users add `.kenn/**` to `[workspace].excludes`.

## Open Questions

- Should the runtime emit a one-shot log line when applying the per-language defaults, like the cleanup-era info log? Probably not — the field is right there in serde-default; users see the default by reading the starter TOML or grepping the crate. The cleanup-era log was justified by the implicit-replacement behavior the field had; per-language defaults are visible at config-definition time.
