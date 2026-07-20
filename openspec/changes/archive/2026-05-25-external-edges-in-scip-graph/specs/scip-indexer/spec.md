## ADDED Requirements

### Requirement: User→external edges are emitted

The SCIP edge-derivation pass SHALL emit edges whose target has zero workspace definitions (`def_count == 0` in the per-run def-count map). The target SHALL be interned via the stub path so it appears in the symbols table. The `def_count > 1` arm SHALL continue to drop occurrences — that filter targets crate-root markers and known producer duplication patterns, and its relaxation is deferred to a separate change with its own evidence base.

#### Scenario: Stdlib reference reaches the graph

- **WHEN** a SCIP `Occurrence` references a target with `def_count == 0` (e.g. `Result::unwrap`)
- **THEN** the edge SHALL be emitted with the enclosing workspace symbol as source and the external symbol as target
- **AND** the target SHALL be interned via the stub path so it appears in the symbols table

#### Scenario: Ambiguous-target reference is still dropped

- **WHEN** an `Occurrence` references a target with `def_count > 1` (e.g. a crate-root marker emitted from multiple files)
- **THEN** the occurrence SHALL be dropped

### Requirement: Drained stubs are tagged external

`flush_registry_stubs` SHALL set `is_external = true` on every `SymbolRecord` it pushes to the sink. A drained stub is by construction a symbol whose full `SymbolFrame` (carrying its definition) never arrived during ingest; such symbols are defined outside the workspace. This holds for both the SCIP and JSONL ingest paths — the JSONL path's existing `pkg_external` plumbing already tags *full* symbols correctly; drain-time tagging closes the stub-only gap on both paths.

#### Scenario: Stdlib symbol drained as external

- **WHEN** the SCIP edge-derivation pass interns a stub for a stdlib symbol (e.g. `core::result::Result::unwrap`) and no document in the run provides a full `SymbolFrame` for it
- **THEN** `flush_registry_stubs` SHALL emit that stub's `SymbolRecord` with `is_external = true`

#### Scenario: Cross-document workspace symbol promoted before drain

- **WHEN** a stub is buffered for a workspace symbol referenced from a document that doesn't define it, and a later document in the same run provides the defining `SymbolFrame`
- **THEN** `mark_full_emitted` SHALL remove the stub from the pending map before drain
- **AND** the symbol SHALL appear in the symbols table with `is_external = false` (from the full record path)

#### Scenario: include_external filter affects SCIP-language results

- **WHEN** an MCP query passes `include_external: false` to `find_symbol` / `search_symbols` / `list_callers` over a Rust workspace
- **THEN** the returned rows SHALL exclude symbols with `is_external = true`
- **AND** the filter SHALL produce results equivalent to the prior behavior (when external edges did not exist in the graph), modulo the absence of any external edges or symbols from the result set
