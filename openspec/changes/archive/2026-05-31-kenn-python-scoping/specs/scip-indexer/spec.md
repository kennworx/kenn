# scip-indexer

## MODIFIED Requirements

### Requirement: Python indexer dispatch via launcher command

When at least one Python unit has been discovered, the indexer driver SHALL invoke `scip-python` once per unit by spawning the configured launcher command (a non-empty sequence of tokens), passing `index --cwd <workspace-root> --output <run-dir>/python-<idx>.scip --quiet` as the trailing arguments where `<idx>` is the unit's 0-based discovery index and `<run-dir>` is the active indexer-pass run directory under the derived store. When the unit's path is a strict descendant of the workspace root (i.e., `[language.python].targets` is non-empty and named that path), the driver MUST additionally pass `--target-only <unit-path>` as a trailing argument. When the configured `project_name` or `project_version` is set, the driver MUST forward each as `--project-name <name>` and `--project-version <version>` respectively for every per-unit invocation. The produced `.scip` outputs SHALL be ingested through the existing SCIP output-parsing requirement and the `PythonTransformer` rewrites `scip-python python <dist> <ver> <descriptor>` symbols into `py:<module>.<...>` public IDs.

This requirement replaces the earlier single-invocation phrasing. The per-target-invocation shape supports monorepo workspaces with multiple Python sub-packages, each invoked with its own `--target-only` for narrowed `TreeVisitor` walks. Pyright analysis state is NOT shared across invocations (scip-python's `Indexer` is a per-process construct); the cost is N × per-target analysis when `targets` has N entries.

#### Scenario: Default launcher with no targets configured

- **WHEN** the user configures `[language.python] enabled = true` with empty `targets`
- **THEN** the driver MUST spawn one `scip-python index --cwd <ws> --output <run-dir>/python-0.scip --quiet` invocation
- **AND** MUST NOT pass `--target-only`

#### Scenario: Single target

- **WHEN** the user configures `targets = ["src/api"]`
- **THEN** the driver MUST spawn one invocation with `--target-only <ws>/src/api` appended

#### Scenario: Multiple targets fan out to N invocations

- **WHEN** the user configures `targets = ["src/api", "src/worker"]`
- **THEN** the driver MUST spawn two `scip-python` invocations, one per target
- **AND** each invocation's `--output` path MUST be distinct (e.g., `python-0.scip` and `python-1.scip`)
- **AND** each invocation MUST include `--target-only` pointing at its target's resolved path

#### Scenario: project_name and project_version forwarded to every invocation

- **WHEN** `project_name = "monorepo"` is set and `targets = ["src/api", "src/worker"]`
- **THEN** both spawned invocations MUST include `--project-name monorepo`

#### Scenario: Launcher routes through bunx (single target)

- **WHEN** the user configures `command = ["bunx", "@sourcegraph/scip-python"]` and `targets = ["src/api"]`
- **THEN** the driver MUST spawn `bunx @sourcegraph/scip-python index --cwd <ws> --output <run-dir>/python-0.scip --quiet --target-only <ws>/src/api`

#### Scenario: Python symbols become py: public IDs

- **WHEN** `scip-python` emits a symbol `scip-python python graphify 0.8.12 graphify/detect/detect_languages().`
- **THEN** the resulting record's public ID MUST be `py:graphify.detect.detect_languages`

## ADDED Requirements

### Requirement: Python multi-target unit discovery

This requirement extends the cross-language *Indexable-unit discovery* requirement with Python-specific multi-target behaviour. The SCIP indexer SHALL discover Python indexable units as follows:

1. If `[language.python].targets` is empty AND at least one `.py` (or `.pyi`) file is present under the workspace root (subject to the existing explicit-exclude globs, git-aware worktree exclusion, and the Python-specific skip set `__pycache__/`, `.venv/`, `venv/`, `.kenn/`), emit exactly ONE unit whose path is the workspace root.
2. If `[language.python].targets` is non-empty, emit ONE unit per entry in the list. Each unit's path SHALL be the workspace-relative target path joined with the workspace root. The Python file existence probe is NOT performed per-target — the user's explicit target list overrides discovery.
3. If `targets` is empty AND no `.py`/`.pyi` file is found after exclusions, emit ZERO units and MUST NOT spawn `scip-python`.

Target paths in `targets` MUST be relative paths interpreted against the workspace root; absolute paths and duplicate entries SHALL be rejected at config load with an error that names the offending entry. Each resolved target path MUST exist as a directory on disk; non-existent targets MUST cause the run to fail in the prepare phase (analogous to the existing `KennDotnet::resolve_projects` behaviour for missing `.sln` paths).

#### Scenario: Empty targets with Python files present

- **WHEN** `targets = []` and the workspace contains `.py` files
- **THEN** the SCIP indexer MUST emit exactly one Python unit at the workspace root

#### Scenario: Empty targets with no Python files

- **WHEN** `targets = []` and the workspace contains no `.py`/`.pyi` files
- **THEN** the SCIP indexer MUST emit zero Python units and MUST NOT spawn `scip-python`

#### Scenario: Targets list bypasses the file existence probe

- **WHEN** `targets = ["src/empty-pkg"]` and the workspace contains `.py` files elsewhere but `src/empty-pkg` itself contains none
- **THEN** the SCIP indexer MUST still emit one unit for `src/empty-pkg`
- **AND** the resulting scip-python invocation MAY produce a near-empty `.scip` — that is the user's explicit choice

#### Scenario: Non-existent target path fails the run

- **WHEN** `targets = ["src/missing"]` and `src/missing` does not exist as a directory
- **THEN** the run MUST fail in the prepare phase with a clear error naming `src/missing`
- **AND** no store write MUST have occurred

#### Scenario: Absolute path in targets rejected at config load

- **WHEN** the user configures `targets = ["/abs/path"]`
- **THEN** config validation MUST reject the file with an error naming `python.targets[0]`

#### Scenario: Duplicate target entries rejected at config load

- **WHEN** the user configures `targets = ["src", "src"]`
- **THEN** config validation MUST reject the file with an error naming the duplicate
- **AND** the run MUST NOT spawn any scip-python invocation

### Requirement: Workspace-relative glob filter at Python ingest

The SCIP→record transform for Python SHALL consult `[language.python].exclude_documents` (a list of workspace-relative glob patterns; default `[]`) before emitting any record from each `scip.Document`. When the list is non-empty, every `Document` whose `relative_path` matches at least one pattern MUST be dropped: no `SymbolRecord`, no `DefRecord`, no occurrence-derived edge is emitted from that document.

Globs are matched against `Document.relative_path` directly using the standard glob crate semantics (`*` non-`/`, `**` recursive). No filesystem normalisation or canonicalisation is performed — scip-python emits `relative_path` as workspace-relative for in-workspace files, which is exactly what the user names in the pattern.

External `SymbolInformation` records emitted by scip-python in its dedicated `scip.Index.external_symbols` frame are NOT affected by this filter — they continue to be ingested through the existing external-symbol path so that in-workspace occurrences referencing symbols defined inside a dropped document still produce edges to external stubs (`is_external = true`).

This filter is independent of and composes with `targets`: `targets` narrows what scip-python's `TreeVisitor` walks (saves scip-python compute); `exclude_documents` narrows what kenn ingests (filters noise without affecting scip-python). Users with sub-directories that `--target-only` cannot exclude (e.g., a `node_modules/` or `__pycache__/` inside a target directory) typically pair the two: `targets = ["src"]` plus `exclude_documents = ["**/node_modules/**"]`.

#### Scenario: Document matching one pattern dropped

- **WHEN** `exclude_documents = ["worked/**"]` AND scip-python emits a `Document` with `relative_path = "worked/httpx/raw/transport.py"`
- **THEN** the SCIP transform MUST NOT emit any `SymbolRecord`, `DefRecord`, or occurrence record from that document
- **AND** the snapshot MUST NOT contain symbols whose definition lives in that document

#### Scenario: Document matching no patterns ingested

- **WHEN** `exclude_documents = ["worked/**"]` AND scip-python emits a `Document` with `relative_path = "graphify/detect.py"`
- **THEN** the SCIP transform MUST ingest every record from that document per the existing requirements

#### Scenario: Multiple patterns, OR-semantics

- **WHEN** `exclude_documents = ["worked/**", "tests/fixtures/**"]`
- **THEN** a `Document` with `relative_path = "tests/fixtures/sample.py"` MUST be dropped
- **AND** a `Document` with `relative_path = "tests/test_detect.py"` MUST be ingested (it matches neither pattern)

#### Scenario: Cross-document edge to a dropped-document symbol still emitted

- **WHEN** `exclude_documents = ["worked/**"]` AND an in-workspace document `graphify/_client.py` has a `ReadAccess` occurrence on a symbol whose Definition lives in the dropped `worked/httpx/raw/transport.py`
- **AND** scip-python's `external_symbols` frame contains the matching `SymbolInformation` for that symbol
- **THEN** the edge from `_client.py` to the symbol stub MUST still be emitted (via the existing external-symbol path)
- **AND** the resulting stub MUST be marked `is_external = true`

#### Scenario: Empty exclude_documents = current ingest behaviour

- **WHEN** `exclude_documents = []` (default)
- **THEN** every `Document` from every per-target `.scip` MUST be ingested per the existing requirements
- **AND** the snapshot's symbol count MUST be identical to the pre-flag behaviour from `kenn-python-support`

#### Scenario: Pattern composes with multi-target

- **WHEN** `targets = ["src/api", "src/worker"]` AND `exclude_documents = ["**/fixtures/**"]`
- **THEN** the filter MUST be applied uniformly to documents from both per-target `.scip` outputs

### Requirement: Python test-marking heuristics

The SCIP→record transform for Python SHALL extend `is_test_descriptor(Language::Python, kind, public_id)` to return `true` when ANY of the following holds on the public_id's native (`py:` prefix stripped, then split on `.`) dotted segments. Several rules carry a leaf/non-leaf distinction to avoid false positives on production identifiers — the Rust arm of `is_test_descriptor` uses the same pattern (`transform.rs:818-822`) for the analogous reason.

1. **Test-directory segment match**: any segment is exactly one of `tests`, `test`, or `__tests__`. When the matching segment is **non-leaf** (i.e., another segment follows it), the rule fires unconditionally. When the matching segment is the **leaf**, the rule fires only when `kind.is_scope()` (Package / Module / Namespace) — preventing a production field or variable literally named `test` from being marked as test, while still catching `py:tests` from `tests/__init__.py` where the module's leaf segment is the directory name itself.
2. **Test-prefix segment**: any segment starts with the literal prefix `test_` (catches `test_detect` modules and `test_handles_redirect` functions).
3. **Test-suffix segment**: any segment ends with the literal suffix `_test` (catches the `foo_test.py` module convention from pytest's `python_files = ["*_test.py"]` discovery). When the matching segment is **non-leaf** (e.g., methods inside a `foo_test.py` module — public_id `py:foo_test.some_method`), the rule fires unconditionally. When the matching segment is the **leaf**, the rule fires only when `kind.is_scope()` — catches the module init for `foo_test.py` itself (public_id `py:foo_test`, kind = Module) while excluding variables and fields literally ending in `_test` (e.g., `previous_test`, `expected_test`). Symmetric to rule 1's leaf scope-kind branch.
4. **Pytest conftest leaf**: the LEAF segment is exactly `conftest`.
5. **Unittest class shape**: the LEAF segment matches a unittest class shape AND `kind.is_class_like()` (`Class` / `Struct` / `Trait` / `Interface` / `Enum` / `TypeAlias`): either starts with `Test` (e.g., `TestParser`) or ends with `Test` / `TestCase` (e.g., `ParserTest`, `ParserTestCase`). The class-shape constraint prevents marking a production field or function literally named `test`.

The function MUST short-circuit on the first matching rule; ordering of evaluation MAY be implementation-defined.

This requirement provides Python's baseline test detection. It runs AFTER the existing file-glob path (`workspace.is_test_path(&relative_path)`) in the transform's `is_test_file || is_test_descriptor(...)` short-circuit, so users who configure `[tests].paths` retain full override authority. Users who DON'T configure `[tests].paths` (today's `TestsConfig::default()` returns an empty list) get conventional Python test marking automatically.

#### Scenario: Module under tests/ directory marked as test

- **WHEN** scip-python emits a function whose public_id is `py:tests.test_detect.test_handles_redirect`
- **THEN** the resulting `SymbolRecord.test` MUST be `true`
- **AND** at least one of rule 1 (non-leaf `tests`) or rule 2 (`test_*` prefix on `test_detect` / `test_handles_redirect`) MUST match

#### Scenario: tests/__init__.py module marked as test (leaf scope-kind fallback)

- **WHEN** scip-python emits the module init for `tests/__init__.py` whose public_id is `py:tests` (single segment, `kind = Module`)
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 1's leaf scope-kind branch
- **AND** the rule MUST NOT fire on the equivalent shape with non-scope kind (see "Production field named `test` NOT marked")

#### Scenario: Module named test_detect at top level

- **WHEN** scip-python emits a class whose public_id is `py:test_detect.TestDetect` AND `[tests].paths` is empty
- **THEN** the resulting `SymbolRecord.test` MUST be `true` (rule 2 on the `test_detect` segment; rule 5 also fires on the `TestDetect` leaf class shape)

#### Scenario: Fixture function inside tests/conftest.py

- **WHEN** scip-python emits a fixture function whose public_id is `py:tests.conftest.client_fixture` AND `kind = Function`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 1's non-leaf branch on the `tests` segment
- **AND** rule 4 MUST NOT fire here (rule 4 requires the leaf to be exactly `conftest`; the leaf is `client_fixture`)

#### Scenario: conftest.py module init at top level (rule 4 leaf match)

- **WHEN** scip-python emits the module init for `conftest.py` at the workspace root whose public_id is `py:conftest` AND `kind = Module`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 4 (leaf is exactly `conftest`)
- **AND** rule 4 is the sole reason — rule 1 does not match (`conftest` is not in {`tests`,`test`,`__tests__`}); rule 2/3 prefix/suffix don't fire; rule 5 requires class-like kind

#### Scenario: unittest TestCase subclass in non-test file

- **WHEN** scip-python emits a class `py:graphify.smoke.SmokeTestCase` AND `kind` is class-like
- **THEN** the resulting `SymbolRecord.test` MUST be `true` (rule 5: leaf ends with `TestCase`, class kind)

#### Scenario: Test class with `Test` prefix in non-test file (rule 5 starts-with branch in isolation)

- **WHEN** scip-python emits a class `py:graphify.TestParser` AND `kind = Class`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 5's "leaf starts with `Test`" branch
- **AND** rule 5 is the sole reason — rule 1 doesn't match (`TestParser` is not in {`tests`,`test`,`__tests__`}); rule 2's `test_` prefix doesn't match (`TestParser` starts with capital `T`, no underscore); rule 3 doesn't fire (no `_test` suffix); rule 4 requires literal leaf `conftest`

#### Scenario: foo_test.py module init marked as test (rule 3 leaf scope-kind branch)

- **WHEN** scip-python emits the module init for `foo_test.py` whose public_id is `py:foo_test` AND `kind = Module`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 3's leaf scope-kind branch
- **AND** rule 3 is the sole reason — rule 1 doesn't match (`foo_test` is not in {`tests`,`test`,`__tests__`}); rule 2's `test_` prefix doesn't fire (`foo_test` doesn't start with `test_`); rule 4 requires literal leaf `conftest`; rule 5 requires the leaf to start with `Test` or end with `Test`/`TestCase`

#### Scenario: Method inside foo_test.py marked as test (rule 3 non-leaf)

- **WHEN** scip-python emits a method `py:foo_test.helper_function` AND `kind = Function`
- **THEN** the resulting `SymbolRecord.test` MUST be `true` via rule 3's non-leaf branch on `foo_test`
- **AND** the rule fires regardless of `kind` here because `foo_test` is non-leaf (leaf is `helper_function`)

#### Scenario: Production field named `test` NOT marked

- **WHEN** scip-python emits a field whose public_id is `py:graphify.config.test` (a config flag) AND `kind = Field`
- **THEN** the resulting `SymbolRecord.test` MUST be `false`
- **AND** the reasoning is: rule 1's leaf-segment branch requires `kind.is_scope()` and `Field` is not scope; rule 2/3 prefix/suffix don't fire on bare `test`; rule 4 requires the literal leaf `conftest`; rule 5 requires class-like kind

#### Scenario: Variable ending in `_test` NOT marked

- **WHEN** scip-python emits a module-level variable whose public_id is `py:graphify.runner.previous_test` AND `kind = Variable`
- **THEN** the resulting `SymbolRecord.test` MUST be `false`
- **AND** the reasoning is: rule 3 (`_test` suffix) is restricted to non-leaf segments; `previous_test` is the leaf

#### Scenario: User's [tests].paths override retains precedence

- **WHEN** the user configures `[tests].paths = ["foo/**"]` and scip-python emits a `Document` with `relative_path = "foo/bar.py"`
- **THEN** every symbol in `foo/bar.py` MUST be marked test via the file-glob, regardless of whether any descriptor rule fires
- **AND** the descriptor heuristic MUST NOT need to be consulted (the file-level glob short-circuits per the existing `is_test_file || is_test_descriptor(...)` evaluation order)
