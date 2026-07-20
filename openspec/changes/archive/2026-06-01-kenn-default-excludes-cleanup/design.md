## Context

`DEFAULT_EXCLUDES` in `crates/kenn-indexer/src/canonicalize.rs:63` is a static slice of eight glob patterns. It is layered into every Workspace's `excludes` GlobSet at construction (`Workspace::new`), then consulted by:

- `walk` → driver discovery walks (skip during recursion).
- `Workspace::canonicalize` → rejects matched files with `CanonicalizeError::Excluded` (the pipeline maps this to "drop, no error").

Today's contents:

```text
node_modules/**, **/node_modules/**   // Node / TypeScript
bin/**, **/bin/**                     // .NET MSBuild
obj/**, **/obj/**                     // .NET MSBuild
target/**, **/target/**               // Rust / Cargo
```

There is no Python entry. `__pycache__/`, `.venv/`, `venv/`, etc. are filtered ad-hoc inside `ScipPython::discover_units` via a `skip_leaves` argument to `walk_skipping` — a discovery-time only filter, NOT a canonicalize-time one. A `__pycache__/foo.py` document arriving from a SCIP file (say from an out-of-band scip-python invocation) would be canonicalized successfully and ingested. Inconsistent with `node_modules/foo.js`.

## Goals / Non-Goals

**Goals:**
- Drop `DEFAULT_EXCLUDES` from compiled code. Move its content + Python additions into the starter `kenn.toml`'s `[exclude].globs` (visible, editable, removable).
- Apply the same default set to fresh workspaces that lack a `kenn.toml` or set `[exclude]` without `globs`: layer in the curated fallback at `kenn_config::Config::validate` (or load), with one-shot `tracing::warn`.
- Workspace handling becomes single-layer: `Workspace.excludes` is built from `user_globs` only; no implicit additions inside `Workspace::new`.
- Move `ScipPython`'s ad-hoc `skip_leaves` defaults into the canonical config-layered list. Once present in `[exclude].globs`, the driver's discovery walk picks them up via `walk_skipping`'s integration with `workspace.excluded_dirs()` / the exclude GlobSet.
- Keep `[language.python].exclude_documents` (from `kenn-python-scoping`) as the per-language ingest-time filter — orthogonal to this change.

**Non-Goals:**
- Promoting `[exclude].globs` to apply at SCIP-document ingest for all languages. That's the cross-language behaviour change `kenn-python-scoping` explicitly scoped to Python.
- Per-language exclude blocks like `[language.python].walk_excludes`. The shared `[exclude].globs` is sufficient and matches user expectations ("workspace-wide skip list").

## Decisions

### Drop the hardcoded constant; layer the default at config-load

`crates/kenn-config/src/lib.rs::ExcludeConfig::globs` already serdes a `Vec<String>` with `#[serde(default)]`. Change: at `Config::validate`, if `self.exclude.globs.is_empty()`, replace with the curated fallback and emit a one-shot `tracing::warn` naming the fallback's content + how to opt out (`globs = []` explicitly). The fallback list lives as a `const` in `kenn-config`, not `kenn-indexer`, so the indexer reads only `config.exclude.globs` (already non-empty when it arrives).

**Why fallback at config load, not Workspace::new**: keeps Workspace's contract simple ("uses exactly what user_globs gave you"); the policy lives next to the rest of the config defaults; the warning fires at a predictable point.

### Default list — content

The fallback list is the union of today's `DEFAULT_EXCLUDES` plus Python conventions:

```text
node_modules/**         # Node / TS
**/node_modules/**
bin/**                  # .NET MSBuild
**/bin/**
obj/**
**/obj/**
target/**               # Rust / Cargo
**/target/**
__pycache__/**          # CPython bytecode cache
**/__pycache__/**
.venv/**                # uv / venv convention
**/.venv/**
venv/**                 # alternate venv name
**/venv/**
.tox/**                 # tox isolated env
**/.tox/**
dist/**                 # python build artefacts
**/dist/**
build/**                # python build artefacts
**/build/**
*.egg-info/**           # setuptools egg metadata
**/*.egg-info/**
```

`.git/` is intentionally NOT here — Workspace's `excluded_dirs()` already handles it via the git-aware-worktree path. `.kenn/` likewise — it's the derived store and is handled via the layout layer.

### `ScipPython` driver: drop ad-hoc skip_leaves

After this change applies, `[exclude].globs` covers the Python skip set. `ScipPython::discover_units` reverts to `walk(workspace.root(), workspace.excluded_dirs())` (the standard helper) — same shape as `KennDotnet` and `RustAnalyzer`. The Python-specific `walk_skipping` site goes away.

This depends on `kenn-python-scoping` landing first because `kenn-python-scoping` reshapes `discover_units` to handle `targets`. The cleanup applies on top of the scoping change.

### Migration

Existing workspaces with NO `[exclude]` section, or `[exclude]` without `globs`, get the fallback automatically — same behaviour as today plus the Python additions. No action needed.

Workspaces with `[exclude].globs = ["custom/**"]` get ONLY their custom list. Today, the hardcoded `DEFAULT_EXCLUDES` silently merged in; after this change, they don't. The starter `kenn.toml`'s `[exclude]` section will document the fallback so users who explicitly set `globs` can copy the entries they want.

This is a small behaviour change; users with `globs = ["custom/**"]` who relied on `node_modules` being implicitly excluded will start seeing `node_modules/` files in the index. The migration note in this section gets surfaced in CHANGELOG when the change lands.

## Risks / Trade-offs

- **One-shot warning is noisy on default-config workspaces** — Mitigation: emit at `INFO` not `WARN`, or gate behind `RUST_LOG=kenn_config=info`. Or omit the warning entirely and accept the implicit-default behaviour. (Recommend: emit at `INFO` with a clear "to opt out, set `globs = []`" line.)
- **Removing implicit-merge is a soft breaking change for existing users** — Acknowledged in Migration; mitigated by the starter `kenn.toml` documenting every default so users see what they'd be opting out of.
- **Python additions might over-exclude in a Python-monorepo** (e.g. a workspace where `build/` is the actual source dir, not a setuptools output) — Mitigation: users override by setting their own `[exclude].globs` with the entries they want. The starter file is the discoverable place to start.

## Open Questions

- Does the curated fallback go in `kenn-config` or `kenn-indexer`? Recommend `kenn-config` so the fallback travels with the config schema (consumers see one source of truth).
- Should we surface the resolved-after-fallback exclude list via `kenn status`? Useful for debugging; not blocking this change. Tracked as a follow-up.
