## ADDED Requirements

### Requirement: Navigation tools default to excluding test symbols

The graph-navigation tools SHALL default `include_tests` to `false`, matching
the search tools — one universal default across the whole surface. This covers
`list_callers`, `list_callees`, `list_implementers`, `list_overrides`,
`list_usages`, `list_correspondences`, `list_in_scope`, and `list_imports`
(previously `true`), alongside `find_symbol`, `search_symbols`, and
`find_similar` (already `false`). A caller SHALL opt in per call with
`include_tests: true` — for example, to include test callers when scoping a
refactor. `include_external` SHALL likewise default to `false`.
`list_module_files` is exempt: it returns every file and flags `test` /
`external` per row rather than filtering.

#### Scenario: list_callers excludes test callers by default

- **WHEN** `list_callers(id)` runs with no filters
- **THEN** symbols defined in test files are omitted from the callers

#### Scenario: include_tests includes test callers

- **WHEN** `list_callers(id, filters: { include_tests: true })` runs
- **THEN** callers defined in test files are included
