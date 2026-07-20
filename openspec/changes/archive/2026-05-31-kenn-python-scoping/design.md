## Context

`ScipPython` (added by `kenn-python-support`) discovers exactly one Python unit per workspace and invokes `scip-python index --cwd <ws>` with no scoping flag. scip-python then runs Pyright twice (`Parse and search for dependencies`, then `Analyze project and dependencies`) and finally emits one `scip.Document` per file via `TreeVisitor`. Critically — **verified by reading `indexer.ts:195-215` and grepping the actual graphify SCIP** — TreeVisitor only walks `projectSourceFiles`, which is filtered to files whose absolute path starts with `projectRoot` (i.e., `cwd`). So scip-python never emits Documents for files outside `cwd`.

This invalidates a tempting predicate that an earlier draft of this design carried: "drop Documents whose `relative_path` is absolute or starts with `..`." Empirically that predicate never fires:

- **graphify SCIP**: 91 documents, all relative under `graphify/`, `tests/`, `worked/`. Zero documents with `..` or absolute paths. Symbol package distribution: 12,374 `graphify` + 4,224 `python-stdlib`. Zero httpx/requests/etc.
- **django SCIP**: 2,909 documents, all under `django/`, `tests/`, top-level config files. Same shape.

The actual problem on graphify is that `worked/httpx/raw/*.py` is **in-workspace** fixture code — committed `.py` files containing httpx-style class definitions — that the user doesn't want in the index. scip-python rightly emits Documents for them (they're under cwd), and they get the `graphify` package name. The fix has to be a kenn-side filter that names paths inside the workspace, not a predicate on relative-path syntax.

Two scoping needs remain valid:

1. **One target subdirectory**: monorepo with a single Python sub-package. `--target-only src/api` covers it in one scip-python invocation.
2. **Multiple targets**: workspace with several independent Python sub-packages. scip-python's `--target-only` accepts at most one path per invocation (verified in `pyright-scip/src/indexer.ts:108-118`), so multiple targets ⇒ N invocations.

Composed with the workspace-relative glob filter, the user has two layered scoping knobs:

- `targets` → which directories scip-python's `TreeVisitor` walks.
- `exclude_documents` → which Documents from those walks are kept at ingest.

## Goals / Non-Goals

**Goals:**
- Add `targets: Vec<String>` and `exclude_documents: Vec<String>` to `[language.python]`. Existing users see no change with default empty lists.
- Honour `targets` via N scip-python invocations, one per entry, each with `--target-only <path>`. Empty list = single invocation, no flag (today's behaviour).
- Drop, at SCIP→record ingest, every `Document` whose `relative_path` matches at least one glob in `exclude_documents`. Cross-document edges from kept Documents to symbols defined inside a dropped Document are preserved via the existing `external_symbols` path.
- Document the per-target Pyright re-analysis cost so users know N targets ≠ N × free.

**Non-Goals:**
- Sharing Pyright state across multiple `--target-only` runs. scip-python's `Indexer` is per-process; sharing would require a daemon.
- Promoting kenn's existing `[exclude].globs` to apply at SCIP-document ingest for all languages. That's a cross-language behaviour change worth its own focused proposal; this change scopes the filter to Python.
- Inferring `targets` from `pyproject.toml` `[project.packages]`. Operator policy; can land later.
- Filtering at scip-python's TreeVisitor itself (would require a fork).
- Touching C# / Rust / TypeScript.

## Decisions

### `targets: Vec<String>` shape mirrors `[language.csharp].projects`

Same field-name semantics (workspace-relative paths, empty = walk default). Differs in cost model: C# batches multiple projects in one `kenn-dotnet` invocation (Roslyn shares the binder); Python can't (Pyright is per-process). The shape lets users name the targets they want; the cost is documented honestly in the field's doc-comment.

**Alternative considered**: `target_only: Option<String>` — single target, no N-invocation cost surprise. Rejected because the multi-sub-package monorepo case is real (every sub-package indexed, no top-level walk). Single-target users use `targets = ["src/api"]` and pay no extra cost.

### Per-target unit, one scip-python per unit, distinct output slug

`ScipPython::discover_units` returns a `Vec<Unit>`. Empty `targets` ⇒ one unit at workspace root (today's behaviour, slug `python-0`). Non-empty ⇒ one unit per entry, each `Unit::path` set to the resolved target dir, slug `python-<idx>` matching the entry index. The existing `IndexerDriver::run_all` loop spawns one `scip-python` per unit and merges their `.scip` outputs through the existing per-unit ingest path. No orchestration changes.

For non-empty `targets`, `run_unit` adds `--target-only <unit.path>` to the spawned command.

**Filename rename**: today's single `python.scip` slug becomes per-unit `python-0.scip` (or `-1`, `-2`, etc.) under the active run directory (`.kenn/local/runs/<timestamp>/`). These are throwaway intermediates; no user-visible artefact moves.

### `exclude_documents: Vec<String>` of workspace-relative glob patterns

Default `[]`. When non-empty, the SCIP→record transform consults the list before emitting any record from each Document. Match semantics: `glob` crate's `Pattern::matches` against the `Document.relative_path` (which scip-python emits as workspace-relative — verified). A document matched by any pattern is dropped entirely (no `SymbolRecord`, no `DefRecord`, no occurrence-derived edges).

**Why workspace-relative**: scip-python always emits relative paths for in-workspace files; the user thinks in workspace-relative terms (`worked/**`, `tests/fixtures/**`); the glob matches the wire encoding directly without normalisation.

**Why drop the whole Document, not individual occurrences**: a Document either represents code the user wants indexed or not. Partial dropping (keep some symbols, drop others) creates inconsistent FROM attribution downstream. Dropping the document is the clean cut.

**External symbol stubs preserved**: scip-python's separate `external_symbols` frame is unaffected by this filter. So if `worked/httpx/raw/transport.py` is dropped but `graphify/_client.py` (kept) references `BaseTransport` defined inside it, the external-stub for `BaseTransport` still lands as `is_external = true`, and the edge from `_client` to that stub is still emitted. The user gets a clean snapshot scoped to project code, with cross-references to dropped code visible as external links.

**Alternative considered**: extend kenn's top-level `[exclude].globs` to apply at SCIP-document ingest for all languages. Cleaner unification but cross-cutting — it would change Rust and TS behaviour too. Worth its own focused change. Scoped to Python here.

**Alternative considered**: filter at the occurrence level rather than the document level. Rejected because Definition occurrences inside a dropped document are the right thing to drop; in-workspace occurrences with `ReadAccess` on symbols defined in dropped documents are legitimate signal (the edge from project code to a now-external symbol), and the existing `external_symbols` path already handles them.

### Composition: `targets` × `exclude_documents`

Both knobs apply independently. `targets = ["src"]` narrows what scip-python walks; `exclude_documents = ["worked/**"]` narrows what kenn ingests. They compose naturally — e.g. `targets = ["."]` (whole workspace) + `exclude_documents = ["worked/**", "tests/fixtures/**"]` keeps scip-python's behaviour identical to today while filtering out the noise at ingest. Or `targets = ["graphify"]` alone, which prevents scip-python from walking `worked/` and `tests/` in the first place — cheaper because Pyright doesn't analyse them.

### Per-language scoping vs cross-language

`exclude_documents` lives under `[language.python]` not `[exclude]`. Reasoning: kenn's top-level `[exclude].globs` is currently a *discovery-time* filter (applied during workspace walking before an indexer is spawned), and promoting it to *also* apply at every language's SCIP-document ingest is a meaningful behaviour change that deserves its own proposal. Scoping to Python here keeps blast radius small. A future cross-language promotion is a one-liner refactor.

### Python test-marking heuristics

The transform calls `workspace.is_test_path(&rel)` (file-glob, configured) and `is_test_descriptor(language, kind, public_id)` (per-language heuristic) when deciding `SymbolRecord.test`. For Python today, the descriptor heuristic returns `false` unconditionally — so users without `[tests].paths` configured see zero test marking, even on workspaces with conventional `tests/test_*.py` layouts. scip-python itself never emits the SCIP `Test` role bit (verified across graphify and django: every occurrence is role 1 or 8 only).

Extend `is_test_descriptor(Language::Python, kind, public_id)` to match the Python ecosystem's conventions. Five rules, with leaf vs non-leaf distinctions mirroring the existing Rust arm pattern at `transform.rs:818-822` (which the Rust arm uses for *the same reason*: guard against fields/fns named `test` in production code):

1. **Test-directory segment** (`tests` / `test` / `__tests__`): non-leaf hits fire unconditionally (a directory in the module path is reliable signal); leaf hits fire only when `kind.is_scope()` (Package / Module / Namespace), so `tests/__init__.py` → `py:tests` (kind = Module) gets marked, but a production field named `test` does not.
2. **`test_` prefix**: any segment qualifies. Functions/methods named `test_*` are conventionally test entry points (pytest, nose2, green); modules named `test_*.py` are pytest discovery targets. False-positive risk on production identifiers starting with `test_` is low and acceptable.
3. **`_test` suffix**: symmetric to rule 1. Non-leaf segments ending in `_test` fire unconditionally (covers methods inside a `foo_test.py` module like `py:foo_test.helper`); leaf segments ending in `_test` fire only when `kind.is_scope()` (catches `foo_test.py`'s module init `py:foo_test` with `kind = Module` — pytest's `python_files = ["*_test.py"]` discovery convention — while excluding variables and fields like `previous_test` whose kind isn't scope).
4. **`conftest` leaf**: pytest's fixture file. Specific to that framework; low false-positive risk.
5. **Class-shape test name** (leaf + class-like kind): leaf starts with `Test` (`TestParser`, unittest convention) or ends with `Test` / `TestCase` (`ParserTest`, `ParserTestCase`, Django/unittest variant). The class-kind constraint (`Kind::is_class_like` — promoted to the model crate by this change since `aggregate.rs::is_class_like` is currently module-private) prevents marking a production method or field literally named `test`.

Path-based detection via `[tests].paths` keeps precedence: when the file is already marked test by glob, every symbol inherits it via `is_test_file || is_test_descriptor(...)` short-circuit. The descriptor heuristic only fires for files not caught by the glob — letting users override the heuristic by adding a `non-test/**` exception to `[tests].paths` if needed (today's `is_test_path` behaviour is unchanged).

**Alternative considered**: ship Python-specific default test path globs (`**/test_*.py`, `**/*_test.py`, `tests/**`) in `TestsConfig::default`. Rejected because `TestsConfig` is cross-language and the existing contract says "Authoritative — no built-in fallback." Adding language-specific defaults at that layer would surprise C# users. Per-language descriptor heuristics (analogous to the existing Rust and Go arms) keep the boundary clean.

**Alternative considered**: read the SCIP `SymbolRole.Test = 32` bit. Reasonable in principle, but scip-python never emits it; the code would never fire. Not worth wiring until an upstream indexer actually starts setting it.

### Duplicate target entries

`targets = ["src", "src"]` is rejected at config load with an error naming the duplicate. Two identical scip-python runs producing the same Documents wastes both compute and storage; the IDRegistry would dedupe at ingest but only after paying the work twice. Cleaner to fail fast.

### `--target-only` does not apply additional excludes

scip-python's `--target-only` is a TreeVisitor walk-scope filter; it does NOT honour Pyright's exclude list nor kenn's `[exclude].globs`. If the user sets `targets = ["src"]` and `src/` contains a `node_modules/` or `__pycache__/`, scip-python walks those too. The ingest-time `exclude_documents` is the recommended companion for this case — it lets the user say `exclude_documents = ["**/node_modules/**", "**/__pycache__/**"]` even when `--target-only` is in use.

## Risks / Trade-offs

- **N targets = N × Pyright analysis** → Mitigation: per-target documentation in the config field's doc-comment. Users with one Python package use one target; users with multiple sub-packages opt in knowingly.
- **`exclude_documents` defaults to empty (no behaviour change)** → Users with the kenn-python-support graphify case still see httpx fixture symbols until they configure the exclude. Trade-off is to keep the kenn-python-support default behaviour unchanged for this additive change. Operator policy.
- **`--target-only` doesn't stop Pyright dep analysis** → Acknowledged; orthogonal to this change. Pyright analyses imported dep files for type-resolution purposes regardless of `--target-only`; those analyses don't produce Documents (only external_symbols), so they don't affect the snapshot symbol count except through cross-reference stubs.
- **Multi-target overlap edge cases** — `targets = ["src", "src/api"]` produces two overlapping `.scip` files. The IDRegistry interns by `(language, pub_id)` (per `code-intel-data-model` D7); duplicates collapse at ingest. No new dedup logic needed beyond what's already in place. Worth one test.

## Migration Plan

Additive — no migration. Defaults reproduce today's behaviour. Users who indexed Python with `kenn-python-support` opt in by adding `targets`, `exclude_documents`, or both to `[language.python]`.

For the graphify case specifically: `exclude_documents = ["worked/**"]` removes the httpx-fixture noise without losing test coverage; or `targets = ["graphify"]` plus `exclude_documents = ["tests/fixtures/**"]` if tests are wanted but their fixtures aren't.

## Open Questions

None — both fields have clear semantics, evidence-grounded defaults, and verified-on-real-data scenarios.
