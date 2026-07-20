## ADDED Requirements

### Requirement: find_usages excludes tests and external references by default

`find_usages` SHALL accept optional boolean `include_tests` and
`include_external` parameters, each defaulting to `false`. With neither set it
SHALL exclude references from test files and references to/from external
(stdlib / vendored) symbols. `include_tests: true` SHALL include test reference
sites — the full refactor surface; `include_external: true` SHALL include
external references. This is the same universal default the search and
navigation tools use, so a bare `find_usages` no longer returns test call
sites (it previously always did).

#### Scenario: default excludes test references

- **WHEN** `find_usages("OrderHandler")` runs with no include flags
- **THEN** references located in test files are omitted from the result

#### Scenario: include_tests recovers the full refactor surface

- **WHEN** `find_usages("OrderHandler", include_tests: true)` runs
- **THEN** references located in test files are included
