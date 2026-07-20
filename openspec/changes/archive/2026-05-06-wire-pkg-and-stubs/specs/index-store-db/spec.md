## ADDED Requirements

### Requirement: packages table interned by (name, version)

The schema SHALL define a `packages` table:

```
packages
  short_id   u32  PRIMARY
  name       string
  version    string?
  manager    string?
  external   bool default false
  UNIQUE INDEX (name, version)
```

The unique index on `(name, version)` enforces the consumer's intern
key. Packages with same `name` and same `version` from different
producer runs (or different on-wire `PackageFrame`s within one run)
collapse to one row.

Different `version`s of the same `name` produce distinct rows
(`Newtonsoft.Json` v12 and v13 are separate packages).

#### Scenario: Two PackageFrames with identical (name, version) collapse

- **WHEN** the wire stream emits two `PackageFrame`s, both with
  `name: "Web"` and `version: "1.0"`
- **THEN** the `packages` table MUST contain exactly one row matching
  `name = 'Web' AND version = '1.0'`
- **AND** any `SymbolFrame` referencing either wire id MUST resolve to
  that single row

### Requirement: defs table for declaration sites

The schema SHALL define a `defs` table:

```
defs
  sym_id      u32                 // FK → symbols.short_id
  file_id     u32                 // FK → files.short_id
  start_line  u32
  start_col   u32
  end_line    u32
  end_col     u32
  INDEX ON sym_id
  INDEX ON file_id
```

One row per declaration site. The common case (non-partial symbol)
produces one `defs` row per `symbols` row. Partial classes/methods
produce N `defs` rows per `symbols` row, all sharing `sym_id`.

Lines and columns SHALL be stored as separate primitive columns so
that callers projecting only line data (the `path#start-end` rendering
case) can omit column projection.

#### Scenario: Non-partial symbol has exactly one defs row

- **WHEN** the consumer ingests a single `SymbolFrame` for a
  non-partial symbol
- **THEN** the `symbols` table MUST contain one row for that symbol
- **AND** the `defs` table MUST contain exactly one row with
  `sym_id` matching that symbol's `short_id`

#### Scenario: Partial class produces one symbols row and N defs rows

- **WHEN** the consumer ingests three `SymbolFrame`s with `partial:
  true`, distinct wire ids, and matching `(key, pkg)`
- **THEN** the `symbols` table MUST contain exactly one row for the
  symbol
- **AND** the `defs` table MUST contain three rows, all sharing
  `sym_id`, with `file_id`/`start_line`/`end_line` reflecting the
  three declaration sites

#### Scenario: Project line-only without column data

- **WHEN** an MCP query renders a symbol location as
  `path#start_line-end_line`
- **THEN** the query MUST be expressible as
  `SELECT file_id, start_line, end_line FROM defs WHERE sym_id = $id`
- **AND** the column data MUST NOT be fetched

## MODIFIED Requirements

### Requirement: symbols table layout

The `symbols` table SHALL include:

```
symbols
  short_id     u32  PRIMARY
  pub_id       string  B-tree (non-unique)
  pkg          u32   default 0
  kind         string
  name         string
  parent       u32   default 0
  partial      bool  default false
  nargs        u32   default 0
  targs        u32   default 0
  test         bool  default false
  external     bool  default false
  sig          string?
  doc          string?
```

The previous `file: ref<files>` and `def_range: array<int>` columns are
removed; declaration locations live in the `defs` table.

The `pub_id` column loses its UNIQUE constraint. Different rows MAY
share `pub_id` when they belong to different packages (e.g., two
versions of the same library that both declare
`Newtonsoft.Json.JsonConvert`). The non-unique B-tree from the
`symbol-search-redesign` proposal is retained for exact and prefix
queries.

`pkg` is a non-nullable `u32` with `0` as the "no package" sentinel
value (matching `REF_NONE`). Consumers MAY filter `pkg != 0` when only
package-scoped symbols are of interest.

`external` is a denormalized boolean derived from
`packages[pkg].external` at insert time. Symbols with `pkg = 0` SHALL
have `external = false`.

#### Scenario: Two rows can share pub_id when packages differ

- **WHEN** the consumer ingests two `SymbolFrame`s with the same `key`
  but `pkg` resolving to two different `packages` rows
- **THEN** the `symbols` table MUST contain two rows
- **AND** both rows MUST have the same `pub_id` value
- **AND** the rows MUST differ in `pkg`

#### Scenario: external is derived from package

- **WHEN** the consumer inserts a symbol whose resolved package has
  `external: true`
- **THEN** the new `symbols` row's `external` column MUST be `true`

#### Scenario: pkg = 0 means no package

- **WHEN** a `SymbolFrame` arrives with `pkg` omitted
- **THEN** the corresponding `symbols` row MUST have `pkg = 0`
- **AND** `external` MUST be `false`

## REMOVED Requirements

### Requirement: symbols.pub_id UNIQUE

**Reason:** Different versions of the same logical package can
legitimately declare symbols sharing the same `pub_id` (`pub_id`
no longer encodes the assembly name). Uniqueness is `(pub_id, pkg)`,
enforced as ingest policy by the consumer's intern logic, not by a DB
constraint.

**Migration:** Drop the UNIQUE index. Retain the non-unique B-tree.
No data migration; reindex.

### Requirement: symbols.file and symbols.def_range columns

**Reason:** Declaration locations move to the `defs` table to support
multiple declaration sites per symbol (partial classes) and to enable
two-phase reads that omit range data when only metadata is needed.

**Migration:** Drop the columns from `symbols`. Insert one `defs` row
per declaration site at ingest. No data migration; reindex.
