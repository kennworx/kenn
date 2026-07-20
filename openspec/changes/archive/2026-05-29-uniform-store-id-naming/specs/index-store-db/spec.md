## ADDED Requirements

### Requirement: Uniform id and FK column naming across core datasets

Across the core entity and relation datasets, id columns SHALL follow one rule: a table's own numeric, run-volatile key SHALL be named `id`; a stable, API-visible string identity SHALL be named `pub_id`; and a numeric foreign key SHALL be named `<role>_id`. The concrete column names SHALL be:

- `symbols`: `id` (was `short_id`), `pub_id` (unchanged), `pkg_id` (was `pkg`), `enclosing_sym_id` (was `enclosing_symbol`).
- `symbol_docs`: `sym_id` (was `symbol`).
- `defs`: `sym_id`, `file_id` (unchanged).
- `edges`: `src_id` (was `source`), `target_id` (was `target`), `corr_canon_id` (was `corr_canonical`).
- `files`: `id` (was `short_id`), `path` (unchanged — see exception below).
- `packages`: `id` (was `short_id`), `name` (unchanged — see exception below).
- `aggregate_nodes`: `id` (was `short_id`); `aggregate_edges`: `min_id`, `max_id` (were `node_min`, `node_max`).

`src_id` / `target_id` SHALL remain generic (not `sym_id`) because edge endpoints are polymorphic — they reference a symbol, file, or package depending on edge kind. The derived-analysis datasets are out of scope and SHALL be left unchanged.

#### Scenario: Entity keys and FKs use the convention

- **WHEN** the graph datasets are written
- **THEN** every entity table's own key column is `id`
- **AND** every numeric foreign key column ends in `_id` and names the role it references (e.g. `sym_id`, `file_id`, `pkg_id`, `enclosing_sym_id`, `src_id`, `target_id`, `corr_canon_id`, `min_id`, `max_id`)

### Requirement: Domain-meaningful stable identities are exempt from `pub_id`

Where an entity's stable, API-visible identity is already a domain-meaningful string, that column SHALL keep its domain name rather than be renamed to `pub_id`: `files.path` and `packages.name`. The package `(name, version)` pair remains an internal interning key and `version` is not promoted to an identity column.

#### Scenario: Files and packages keep their domain identity names

- **WHEN** the `files` and `packages` datasets are written
- **THEN** the file's stable identity column is `path` and the package's is `name`
- **AND** neither is renamed to `pub_id`
