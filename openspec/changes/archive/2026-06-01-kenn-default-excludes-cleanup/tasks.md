> **Dependency**: applies on top of `kenn-python-scoping`. `ScipPython` discovery is reshaped by that change and this task set assumes the post-scoping shape (multi-target unit discovery).

## 1. Config — curated fallback at load time

- [x] 1.1 In `crates/kenn-config/src/lib.rs`, define `pub const DEFAULT_EXCLUDE_FALLBACK: &[&str]` (~22 entries: Node/.NET/Rust/Python). Doc-comment names the source for each block.
- [x] 1.2 Modify `Config::validate` (or add a `Config::apply_defaults` step ordered before `validate`): when `self.exclude.globs.is_empty()`, replace with the fallback and emit one `tracing::info!(target: "kenn_config::exclude", "applied default exclude fallback ...")` line naming the count of patterns inserted and the opt-out (`globs = []`).
- [x] 1.3 Round-trip tests:
  - `parses_no_exclude_section_yields_fallback` — empty TOML → `config.exclude.globs` is the fallback.
  - `single_non_matching_pattern_opts_out_of_fallback` — `[exclude] globs = ["__never_match__/**"]` → resolved set is exactly the user value, no fallback applied. (Replaces "explicit empty list opts out" — see design.md: distinguishing missing-key from empty-list at serde-level would require breaking the public `globs: Vec<String>` field shape; users opt out by providing any non-empty list.)
  - `explicit_non_empty_globs_replace_fallback` — `[exclude] globs = ["custom/**"]` → exactly `["custom/**"]`.
  - `fallback_contains_python_defaults` — fallback list includes `"__pycache__/**"`, `".venv/**"`, `"*.egg-info/**"`.

## 2. Indexer — drop DEFAULT_EXCLUDES

- [x] 2.1 In `crates/kenn-indexer/src/canonicalize.rs`, remove `pub const DEFAULT_EXCLUDES` and its loop in `Workspace::new`. `Workspace::new` now builds `excludes` from `user_globs` only.
- [x] 2.2 Update doc-comment on `Workspace::new` to state the new contract ("uses exactly the patterns passed in; pre-resolved fallback if any is the caller's job").
- [x] 2.3 Search & remove any test that relies on the implicit-merge behaviour (`canonicalize_excludes_node_modules_implicitly` and friends, if present). Migrate by passing the fallback explicitly in those tests.

## 3. ScipPython — drop ad-hoc skip_leaves

- [x] 3.1 In `crates/kenn-indexer/src/driver/python.rs::discover_units`, replace the per-call `skip_leaves = &[...]` + `walk_skipping(...)` with `walk(workspace.root(), workspace.excluded_dirs())` — the same shape `KennDotnet` / `RustAnalyzer` use.
- [x] 3.2 Verify driver tests still pass with the implicit-fallback policy now sourced from `kenn-config`. The driver-mod tests use `Workspace::new(dir.path(), &[])`; they'll see an empty exclude set — adjust to pass the fallback explicitly where needed, OR convert those tests to `Workspace::new(dir.path(), &kenn_config::DEFAULT_EXCLUDE_FALLBACK.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())`.

## 4. Starter kenn.toml

- [x] 4.1 In `crates/kenn-cli/src/starter_kenn.toml`, update the `[exclude]` section: list every fallback entry (uncommented for the cross-language ones, commented for the Python-monorepo edge cases like `build/**` that some users actively want to keep).
- [x] 4.2 Add a top-comment block in the section: "These are kenn's default exclude globs. Delete what doesn't apply, add what does. Setting `globs = []` opts out entirely."

## 5. Verification

- [x] 5.1 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 5.2 `cargo test -p kenn-config -p kenn-indexer -p kenn-cli` green.
- [x] 5.3 `just crap-ci` no regression.
- [x] 5.4 Re-index `tmp/graphify` with the starter `kenn.toml` (no `[exclude]` section). `kenn status` reports document count unchanged from the pre-cleanup behaviour for everything except: Python conventional dirs (`__pycache__/`, `.venv/`) are now skipped by canonicalize too (verify by greping the pre-vs-post snapshot for any `__pycache__` document — expected zero post-cleanup).
- [x] 5.5 Reindex with `[exclude] globs = ["__never_match__/**"]` (the documented opt-out): confirm `node_modules/**` is NOT skipped (expects more documents than 5.4).
- [x] 5.6 `cargo fmt --all`.
- [x] 5.7 ~~Update CHANGELOG~~ — this repo has no CHANGELOG.md. Migration note already lives in `design.md` under "Migration"; it travels with the archived change for future readers.
