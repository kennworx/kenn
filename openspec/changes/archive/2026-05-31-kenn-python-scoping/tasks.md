> **Sequencing note**: §1 (config) and §2 (driver) must land together in one commit — the new field references in `cmd_index.rs`/`workflow.rs` won't compile until both struct shapes match. §3 (transform) is independent and can land separately. §4 (verification) runs after all logic is in place.
>
> **Dependency**: This change depends on `kenn-python-support` being applied. Until that change is archived, the MODIFIED requirements in this delta target a not-yet-published spec.

## 1. Config — `targets` and `exclude_documents`

- [x] 1.1 In `crates/kenn-config/src/lib.rs`, extend `PythonConfig` with `targets: Vec<String>` (default `vec![]`) and `exclude_documents: Vec<String>` (default `vec![]`). Both fields `#[serde(default)]`; struct keeps `#[serde(deny_unknown_fields)]`. Doc-comment on `targets` MUST mention the N × Pyright cost honestly.
- [x] 1.2 Extend `Config::validate` to reject `python.targets` entries that are absolute paths, with a clear error naming `python.targets[<idx>]` and the offending value.
- [x] 1.3 Extend `Config::validate` to reject duplicate entries in `python.targets`, with a clear error naming the duplicate.
- [x] 1.4 Add `glob` crate dependency to `kenn-config` (for compile-time pattern validation) or to `kenn-indexer` (for runtime matching) — pick whichever crate already has it on its dep tree; both is fine if not. Validate every `python.exclude_documents` pattern at config load via `glob::Pattern::new(&p)`; emit `ConfigError::InvalidGlob { language: "python", index, pattern, reason }` on failure.
- [x] 1.5 Add unit tests for: absolute-path rejection per entry index; duplicate rejection; invalid-glob rejection (e.g. unmatched `[`); `parses_python_targets_and_excludes` round-trip of a populated `[language.python]` block.
- [x] 1.6 Update `defaults_when_empty` to assert both new fields default to empty.

## 2. Driver — multi-unit discovery & `--target-only` forwarding

- [x] 2.1 Extend `ScipPython` struct with `targets: Vec<String>`. Keep `command`, `project_name`, `project_version` as-is. `exclude_documents` does NOT live on the driver — it's consumed by the transform layer.
- [x] 2.2 Update `ScipPython::discover_units`: if `targets` is empty, current behaviour (one unit at workspace root iff `.py`/`.pyi` present, slug `python-0`). Otherwise emit one `Unit { identifier: format!("python-{idx}"), path: workspace.root().join(target) }` per entry, verifying each resolved path exists as a directory (return `DriverError::Subprocess` with a clear message on miss, mirroring `KennDotnet::resolve_projects`).
- [x] 2.3 Update `ScipPython::run_unit`: parse the `idx` from `Unit::identifier` (`python-<idx>`), allocate per-unit `make_scip_output_path(workspace, &format!("python-{idx}"))`. When the unit's path differs from the workspace root, append `--target-only <unit.path>` to the spawned args.
- [x] 2.4 Wire `config.language.python.targets` through `cmd_index::build_driver` and `workflow::configure_runner` into the new `ScipPython.targets` field.
- [x] 2.5 Add driver tests covering the four shapes:
  - `targets = []` with `.py` files → one unit, no `--target-only`.
  - `targets = ["src/api"]` → one unit, `--target-only <ws>/src/api`.
  - `targets = ["src/api", "src/worker"]` → two units, distinct output slugs.
  - `targets = ["missing"]` → discovery returns `DriverError::Subprocess` naming `missing`.

## 3. Transform — workspace-relative glob filter at ingest

- [x] 3.1 Add an `exclude_documents: Vec<glob::Pattern>` field to the Python transform configuration (`crates/kenn-indexer/src/transform.rs` — or a sibling helper). Threaded from `config.language.python.exclude_documents` at construction; patterns are pre-compiled into `glob::Pattern` so per-document match is O(patterns) without re-parsing.
- [x] 3.2 In the document-level transform entry point, short-circuit before any record emission: if any compiled pattern's `matches(&document.relative_path)` returns true, emit zero records and increment a debug-logged counter (so users can see scoping is working).
- [x] 3.3 Verify the existing `external_symbols` frame ingest is untouched — that path is separate from per-document ingest. Cross-document edges from in-workspace occurrences to dropped-document symbols MUST still emit (the stub comes from `external_symbols`).
- [x] 3.4 Wire `config.language.python.exclude_documents` from `cmd_index::build_driver` and `workflow::configure_runner` through to the transform's construction site (which lives below the driver — likely a parameter on the per-document call or on the transform-state struct).
- [x] 3.5 Tests:
  - Document `relative_path = "worked/httpx/raw/transport.py"` with `exclude_documents = ["worked/**"]` → zero records emitted; counter incremented.
  - Same document with `exclude_documents = []` → full ingest.
  - Document `relative_path = "graphify/detect.py"` with `exclude_documents = ["worked/**"]` → full ingest.
  - Document `relative_path = "tests/fixtures/sample.py"` with `exclude_documents = ["worked/**", "tests/fixtures/**"]` → dropped (matches second pattern).
  - Document `relative_path = "tests/test_detect.py"` with the same patterns → ingested (matches neither).
  - An in-workspace document referencing a symbol defined in a dropped document → edge emitted via external_symbols stub; stub has `is_external = true`.

## 4. Python test-marking heuristics

- [x] 4.1 In `crates/kenn-model/src/kind.rs`, promote the `is_class_like` helper from `crates/kenn-indexer/src/aggregate.rs:54` to a public `Kind::is_class_like(self) -> bool` method matching `Class | Struct | Trait | Interface | Enum | TypeAlias`. Update `aggregate.rs::is_class_like` to delegate to it (or remove the local copy and use `Kind::is_class_like` directly at the call sites in `aggregate.rs:96`).
- [x] 4.2 In `crates/kenn-indexer/src/transform.rs::is_test_descriptor`, replace the `Language::TypeScript | Language::Python | Language::Csharp => false,` arm by splitting Python off into its own arm. Strip the `py:` prefix, split on `.`, scan segments per the five rules in the spec:
  - Rule 1: segment in {`tests`, `test`, `__tests__`}. Non-leaf hits return true unconditionally. Leaf hits return true only when `kind.is_scope()` — mirrors the existing Rust arm pattern at `transform.rs:818-822`.
  - Rule 2: segment starts with `test_`. Any segment qualifies (no leaf restriction).
  - Rule 3: segment ends with `_test`. Non-leaf hits return true unconditionally. Leaf hits return true only when `kind.is_scope()` (symmetric to rule 1; catches `foo_test.py` module init at public_id `py:foo_test` with `kind = Module`, while excluding `previous_test` / `expected_test` variables and fields).
  - Rule 4: leaf is exactly `conftest`.
  - Rule 5: leaf starts with `Test` OR ends with `Test` / `TestCase` AND `kind.is_class_like()` (the method promoted in 4.1).
  Short-circuit on first match.
- [x] 4.3 Add tests in the existing `is_test_descriptor_*` test cluster:
  - `is_test_descriptor_python_marks_tests_directory_non_leaf` — `py:tests.test_detect.test_handles_redirect` with `kind = Function` → true.
  - `is_test_descriptor_python_marks_tests_module_init_leaf_scope` — `py:tests` with `kind = Module` → true (rule 1 leaf scope-kind branch).
  - `is_test_descriptor_python_marks_test_prefix_module` — `py:test_detect.TestDetect` with `kind = Class` → true.
  - `is_test_descriptor_python_marks_conftest_fixture` — `py:tests.conftest.client_fixture` with `kind = Function` → true (rule 1 non-leaf `tests`).
  - `is_test_descriptor_python_marks_conftest_module_init` — `py:conftest` with `kind = Module` → true (rule 4 leaf match; isolates the rule 4 branch since no other rule fires).
  - `is_test_descriptor_python_marks_test_case_class` — `py:graphify.smoke.SmokeTestCase` with `kind = Class` → true (rule 5 ends-with `TestCase`).
  - `is_test_descriptor_python_marks_test_prefix_class_in_isolation` — `py:graphify.TestParser` with `kind = Class` → true (isolates rule 5's starts-with `Test` branch; rules 1-4 don't fire).
  - `is_test_descriptor_python_marks_foo_test_module_init` — `py:foo_test` with `kind = Module` → true (isolates rule 3's leaf scope-kind branch).
  - `is_test_descriptor_python_marks_method_in_foo_test_module` — `py:foo_test.helper_function` with `kind = Function` → true (rule 3 non-leaf branch).
  - `is_test_descriptor_python_does_not_mark_production_field_named_test` — `py:graphify.config.test` with `kind = Field` → false (rule 1 leaf-scope branch requires scope kind; Field is not scope).
  - `is_test_descriptor_python_does_not_mark_variable_ending_in_test_suffix` — `py:graphify.runner.previous_test` with `kind = Variable` → false (rule 3 leaf branch requires scope kind; Variable is not scope).
  - `is_test_descriptor_python_does_not_mark_unrelated_symbols` — `py:graphify.detect.detect_languages` with `kind = Function` → false.
- [x] 4.4 Verify the existing `is_test_descriptor_*` Rust/Go/TypeScript/Csharp tests still pass — the change is additive to the Python arm only. The TypeScript and Csharp arms still return `false` (unchanged).
- [x] 4.5 Verify aggregate.rs tests still pass after the `is_class_like` promotion.

## 5. Verification — end-to-end + quality gates

- [x] 5.1 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 5.2 `cargo test -p kenn-config -p kenn-indexer -p kenn-cli -p kenn-mcp` green.
- [x] 5.3 `just crap-ci` no regression on the new driver / transform code.
- [x] 5.4 Re-index `tmp/graphify` with `exclude_documents = ["worked/**"]` and `targets = []`:
  - `kenn status` reports a smaller `symbols` count than the kenn-python-support baseline (2 298). Record the new count.
  - `kenn visualize` produces a REPORT.md where "God Nodes — User (Live)" does NOT contain `BaseTransport`, `HTTPTransport`, `Auth`, etc.
  - REPORT.md's headline now shows a non-zero `test` count (test heuristics kick in on `tests/test_*.py` symbols without any `[tests].paths` config).
- [x] 5.5 Re-index `tmp/graphify` with `targets = ["graphify"]` and `exclude_documents = []`:
  - scip-python's per-file progress log mentions only files under `graphify/`.
  - `kenn status` reports `documents ≈ 27` (matches the count of `graphify/*.py` files verified by `find tmp/graphify/graphify -name "*.py" | wc -l`).
  - No httpx classes in REPORT.md.
- [x] 5.6 Index a single django sub-package with `targets = ["django/contrib/auth"]`:
  - scip-python's per-file progress log mentions only files under `django/contrib/auth/`.
  - `kenn status` reports a much smaller `documents` count than the full-django baseline (2 909).
- [x] 5.7 Index `tmp/django-src` with `targets = ["django/contrib/auth", "django/contrib/admin"]`:
  - Two `.scip` files appear under the run directory (`python-0.scip`, `python-1.scip`).
  - `kenn status` reports a snapshot covering both sub-package directories.
  - Wall-clock is approximately 2 × per-target time, confirming the documented "N targets = N × Pyright analysis" cost.
- [x] 5.8 Compose test: `targets = ["src/api", "src/worker"]` plus `exclude_documents = ["**/fixtures/**"]` on a fixture workspace; confirm the filter applies to both per-target outputs.
- [x] 5.9 Test-heuristic spot check on `tmp/django-src`: `kenn mcp` → `find_symbol("TestCase")` returns Django's `TestCase` and its subclasses, with `test = true` on the SymbolRefs. Confirms rule 5 (TestCase leaf) fires on Django's conventions.
- [x] 5.10 `cargo fmt --all` as the final pre-commit step.
- [x] 5.11 Update `crates/kenn-cli/src/starter_kenn.toml` `[language.python]` block to add one commented example each for `targets` and `exclude_documents`. Keep the file scannable per the `kenn-python-support` policy.
