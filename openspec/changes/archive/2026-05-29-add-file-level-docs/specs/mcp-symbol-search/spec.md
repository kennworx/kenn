## ADDED Requirements

### Requirement: BM25 search returns file hits alongside symbol hits

The symbol-search tools (`search_symbols`, and the code scope of `semantic_search`) SHALL return file-doc hits interleaved with symbol hits, ranked by the same blended score. The reader SHALL partition hits by kind before hydrating: a file hit (empty `pub_id`) SHALL be resolved from the `files` dataset by its `id`, and a symbol hit (non-empty `pub_id`) through the existing symbol hydration. A file hit's `id` SHALL NOT be resolved against `SYMBOLS` — file and symbol ids are independent id spaces, so blind resolution would return an unrelated symbol.

Every hit SHALL carry a single `kind` discriminant: a file hit's `kind` is the literal `"file"`, a symbol hit's `kind` is its symbol subtype (`"class"`, `"method"`, …). A file hit SHALL carry `kind`, `path`, and `score` only (its extension implies the language). A symbol hit SHALL carry `id`, `kind`, `loc`, `score`, and `test` (the last omitted unless `true`); a null `loc` marks an external symbol, so there is no separate flag.

`find_symbol` (literal-name lookup) SHALL be unchanged and SHALL NOT return file hits — files have no symbol name to match.

#### Scenario: A query matching a file header returns a file hit

- **GIVEN** `src/OrderIntake.cs` has a file-level doc `"Handles order intake validation"` indexed as a doc row
- **WHEN** the agent calls `search_symbols` with `order intake validation`
- **THEN** the results include a file hit for `src/OrderIntake.cs` carrying `path` and `score`
- **AND** the hit is marked as a file via `kind == "file"`

#### Scenario: Symbol hits are unaffected

- **WHEN** a query matches both a symbol doc and a file doc
- **THEN** both appear in the ranked results, each carrying its `kind`, ordered by score

#### Scenario: A file hit does not collide with a same-numbered symbol

- **GIVEN** a file with `id` N (file id space) and an unrelated symbol with `id` N (symbol id space)
- **WHEN** a file-doc row for that file is hit by BM25
- **THEN** it hydrates from the `files` dataset by its `id` and returns the file
- **AND** it does NOT resolve to the symbol that happens to share id N

#### Scenario: find_symbol returns no file hits

- **WHEN** the agent calls `find_symbol` with a literal name
- **THEN** only symbol matches are returned; no file rows appear
