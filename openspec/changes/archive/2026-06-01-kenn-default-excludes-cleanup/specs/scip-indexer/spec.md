# scip-indexer

## ADDED Requirements

### Requirement: Workspace exclude-glob fallback at config load

This requirement complements *Indexable-unit discovery* by specifying where the default workspace-walk exclude globs come from. Files under configured **explicit-exclude globs** (sourced from `[exclude].globs` in `kenn.toml`) MUST be skipped at workspace-walk time AND MUST be rejected by `Workspace::canonicalize` (the SCIP→record transform short-circuits the document on rejection).

When `[exclude].globs` is absent or empty, the config layer SHALL substitute a curated fallback list covering the conventional output / cache directories of every supported language (Node `node_modules/`, .NET MSBuild `bin/` and `obj/`, Rust Cargo `target/`, Python `__pycache__/`, `.venv/`, `venv/`, `.tox/`, `dist/`, `build/`, `*.egg-info/`). The fallback substitution SHALL emit a one-shot informational log line at config load naming the patterns it inserted.

When `[exclude].globs` is non-empty, the indexer SHALL use exactly the user-provided list with no implicit additions from any compiled-in default. Linked git worktrees (see *Git-aware worktree exclusion*) remain excluded regardless of `[exclude].globs`.

Users who want to opt out of the fallback set their own non-empty list — even a single trivially-non-matching pattern like `globs = ["__never_match__/**"]` opts out, because the fallback substitution triggers only on an empty list. A truly empty exclude set is not directly expressible; this is an accepted limitation of distinguishing `globs = []` from a missing `[exclude]` section in serde without breaking the public field shape.

This requirement replaces the earlier phrasing that hardcoded the default exclude list inside `crates/kenn-indexer/src/canonicalize.rs::DEFAULT_EXCLUDES`. The defaults move into the starter `kenn.toml` (commented + uncommented) and into a `kenn-config`-side fallback so the policy is visible to users and editable per workspace.

#### Scenario: No exclude section → fallback applies

- **WHEN** the workspace's `kenn.toml` has no `[exclude]` table
- **THEN** config loading MUST substitute the curated fallback list
- **AND** an informational log line MUST name at least one Python entry (e.g. `__pycache__/**`) to confirm Python defaults are present
- **AND** a `node_modules/foo.js` file MUST be skipped by both discovery walk and `Workspace::canonicalize`

#### Scenario: User-supplied globs replace the fallback

- **WHEN** the user configures `[exclude] globs = ["custom/**"]`
- **THEN** the resolved exclude set MUST be exactly `["custom/**"]` — no implicit `node_modules/**`, no implicit `__pycache__/**`
- **AND** a `node_modules/foo.js` file MUST be ingested if no other rule excludes it

#### Scenario: Single non-matching pattern opts out of fallback

- **WHEN** the user configures `[exclude] globs = ["__never_match__/**"]`
- **THEN** the resolved exclude set MUST be exactly `["__never_match__/**"]`
- **AND** no fallback substitution MUST occur
- **AND** the informational log line MUST NOT fire

#### Scenario: Python conventional dirs are excluded by default

- **WHEN** the workspace has no `[exclude]` section AND the workspace contains `__pycache__/foo.cpython-313.pyc`, `.venv/lib/...`, `dist/wheel.whl`
- **THEN** all three paths MUST be skipped at workspace-walk time
- **AND** if a SCIP file references any of them, `Workspace::canonicalize` MUST reject the path
