## ADDED Requirements

### Requirement: def_range Is Populated for Every Non-External Symbol

For every symbol emitted into the `code-intel-data-model` from a SCIP source, the transform SHALL populate `DefRecord.{start_line, start_col, end_line, end_col}` from the `Occurrence` whose `symbol_roles` includes `SymbolRole::Definition` for that symbol in the indexed documents. The transform MUST NOT push a placeholder `[0, 0, 0, 0]` `DefRecord` for symbols that have a definition occurrence in the SCIP file.

Synthetic and external symbols (those without a `Definition` occurrence in any indexed document — typically symbols declared in dependencies the SCIP file references but does not index) MAY have `def_range = [0, 0, 0, 0]`; in that case the symbol MUST also be marked `is_external = true`.

Stored lines are 1-based per the `source-data-model` requirement; the conversion happens during this transform.

#### Scenario: A Rust function declared at file line 10 has non-zero def_range

- **WHEN** a Rust function is indexed and its SCIP `Occurrence` has `symbol_roles & Definition != 0` with `range = [9, 4, 9, 24]` (0-based)
- **THEN** the resulting `DefRecord` MUST have `start_line = 10, end_line = 10`
- **AND** the `defs` row MUST NOT be `[0, 0, 0, 0]`

#### Scenario: An external symbol with no Definition occurrence keeps zero range

- **WHEN** a symbol appears only as a `Reference` (e.g., `std::vec::Vec` used but not defined in the indexed Cargo unit)
- **THEN** the `DefRecord` MAY be `[0, 0, 0, 0]`
- **AND** the symbol MUST be marked `is_external = true`

#### Scenario: A symbol with multiple Definition occurrences (partial / cfg-gated)

- **WHEN** a Rust item has two `Definition` occurrences (e.g., two `#[cfg(...)]`-gated `impl` blocks)
- **THEN** the `defs` table MUST contain one row per `Definition` occurrence
- **AND** all rows MUST share the same `sym_id` with distinct `file_id`/`start_line`
