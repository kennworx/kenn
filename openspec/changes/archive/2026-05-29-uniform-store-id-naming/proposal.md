## Why

The store's id columns are named inconsistently. An entity's own numeric key is `short_id` on some tables but referenced as `symbol`, `sym_id`, `file_id`, `source`, `target`, `pkg`, `node_min`, … elsewhere — with no rule for when an FK gets an `_id` suffix. The search dataset overloads the problem: `short_id` is a *volatile join key* while a separate `id` column is the *stable identity*, and the two names give no hint which is which. This makes the schema hard to read and the reconciliation-vs-join distinction easy to get wrong (it already bit the file-docs design).

This change imposes one naming rule across the core entity, relation, and search datasets so a column's name states its contract.

## What Changes

The convention:
- **`id`** — numeric, **volatile** (rewritten every run), not API-visible, used only for in-run joins. This is today's `short_id`. Every entity table's own key becomes `id`.
- **`pub_id`** — string, **stable** across runs, API-visible identity.
- **`<role>_id`** — numeric FK to another row's `id`.

Rename map (core graph + search; derived-analysis datasets are out of scope):

- Entity PKs `short_id → id` on `symbols`, `files`, `packages`, `aggregate_nodes`.
- `symbols`: `pkg → pkg_id`, `enclosing_symbol → enclosing_sym_id` (`pub_id` unchanged).
- `symbol_docs`: `symbol → sym_id`.
- `defs`: `sym_id`, `file_id` already conformant — unchanged.
- `edges`: `source → src_id`, `target → target_id`, `corr_canonical → corr_canon_id` (`src_id`/`target_id` are polymorphic FKs — they point to a symbol, file, or package depending on edge kind, so they stay generic, not `sym_id`).
- `aggregate_edges`: `node_min → min_id`, `node_max → max_id`.
- **search dataset**: `short_id → id` (volatile polymorphic join key → symbol or file), `id`/`stable_id → embed_key` (the internal composite key used to reconcile/reuse committed embeddings — not API-visible). `pub_id` is **unchanged** — it stays the symbol's API-visible public id (empty on non-symbol rows), so `pub_id` means the same thing store-wide.

Accepted exceptions (stable string identity that is domain-meaningful, not an opaque pub_id):
- `files.path` — a file's stable, API-visible identity is its path.
- `packages.name` — a package's API-visible identity is its name; `(name, version)` is an internal interning key (version not surfaced).

## Capabilities

### Modified Capabilities
- `index-store-db`: uniform id/FK column naming across the graph datasets (entity PKs → `id`, FKs → `<role>_id`).
- `lance-search`: search dataset columns `id` (volatile join), `pub_id` (symbol's API-visible public id, unchanged), `embed_key` (internal composite embedding-reconciliation key).
- `store-layout`: drop the required `SCHEMA_CHANGELOG.md` pairing from the schema-version requirement (the `STORE_SCHEMA_VERSION` constant and strict-equality check are retained for future use).
- `mcp-orchestrated-indexing`: the schema-mismatch error no longer points at `SCHEMA_CHANGELOG.md` (the Failed → recovery-reindex path is unchanged).

## Impact

- **Schema rename across all core datasets + every reader/writer/batch-builder** that references the column-name constants (`COL_*` in graph + lance `schema.rs`) and the `kenn-model` record fields. The schema changes in place and requires a reindex; **no `STORE_SCHEMA_VERSION` bump** (no users yet — we don't carry version-compat while prototyping).
- **No API/wire change intended**: the JSONL wire frame field names (producer side) and the MCP response JSON are out of scope — this is a store-column/record-field rename. (If wire/API alignment is wanted, it's a follow-up.)
- **Drops `SCHEMA_CHANGELOG.md`**: deletes `crates/kenn-store/SCHEMA_CHANGELOG.md` and the `(see SCHEMA_CHANGELOG.md)` references in 4 source files (`api/types.rs`, `lib.rs`, `kenn-mcp/indexing.rs`, `cmd_status.rs`). The changelog discipline is premature with no users. The `STORE_SCHEMA_VERSION` constant, the strict-equality check, and the `SchemaMismatch` → Failed → recovery path are **kept** for future use.
- **Out of scope**: the derived-analysis datasets (`analysis_god_nodes`, `analysis_flat_communities`, `analysis_anchored_hierarchy`, `analysis_node_membership`) and their `community_id` / node-FK columns.
- **Sequencing**: lands **before** `add-file-level-docs`, which then rebases onto the final names (`file_docs.file_id`; search `id`/`pub_id`/`embed_key`).
