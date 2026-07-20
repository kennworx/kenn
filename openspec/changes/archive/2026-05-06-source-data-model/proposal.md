## Why

The `scip-indexing-pipeline` proposal defines how to *produce* normalized code structure records. The `indexed-store-and-lifecycle` proposal defines the *physical* storage layout, atomicity, and ingest pipeline that places records on disk. Both proposals deliberately leave the **logical data model** unspecified — the public ID format, the table/relation shape, the field semantics that consumers see — because the read-side requirements weren't yet articulated.

This proposal defines the **logical source-data-model** that the producer writes into and that any reader (MCP server, web UI, CLI inspector, future LSP bridge) consumes. It is the common contract — the schema and identity rules — that decouples production from consumption.

Naming: it is "source-data-model" rather than "mcp-data-model" because MCP is one of multiple potential consumers; this proposal is the substrate, not a single API.

## What Changes

- Define the **public symbol-ID format**: per-language native syntax with a short language prefix (`cs:` / `rs:` / `ts:` / `go:` / `py:`), encoding the semantic location in a way that survives file renames and code moves where the language permits.
- Define the **internal short-ID strategy**: every cross-reference inside the DB uses `u32` short ids (file_id, short_id), translated to/from public IDs only at the API boundary.
- Define the **schema**: tables (`files`, `symbols`, `symbol_docs`, `partial_defs`) and graph relations (`defined_in`, `contains`, `calls`, `type_use`, `field_access`, `implements`, `overrides`, `instantiates`, `generic_constraint`, `corresponds_to`, `imports`).
- Define the **wire location format**: `./file_path#start-end` (workspace-relative path, line range).
- Define **occurrences** as an ingest-time intermediate, NOT a persisted table — the live DB stores deduplicated relations only.
- Define the **kind enum** (closed set: package, module, namespace, class, struct, interface, trait, enum, method, function, constructor, field, property, constant, parameter, type_parameter, variable, alias, macro).
- Define the **multi-language** and **isomorphism** stance — single symbols table with `language` column; per-language ID transforms; `corresponds_to` relation for cross-language equivalents.
- Define the **test marking** convention: glob-based config in `kenn.toml` populates `files.is_test`, denormed onto `symbols.is_test`.

## Capabilities

### New Capabilities

- `source-data-model`: the normalized logical model that producers write and consumers read. Defines public ID format, kind enum, table shapes, relation kinds, indexes, and the wire location format. Producer-agnostic and consumer-agnostic.

### Modified Capabilities

- `kenn-data-model` (from `scip-indexing-pipeline`): refined to align with this proposal's identity rules. The producer-side data model is now a *write* view of `source-data-model` — it produces records that, after deduplication and FROM-attribution, materialize the schema defined here.

## Impact

- **Decouples producer from storage**: the SCIP indexer pipeline produces records against the `source-data-model` contract; the embedded DB stores them in the schema defined here. Either side can be changed (within contract) without rewriting the other.
- **Decouples storage from API surface**: MCP server (and any other future API) reads this model. Public IDs, kind values, location format are stable; internal short-ids and table layout can evolve without breaking agents holding IDs in their context.
- **Multi-language by default**: schema is multi-language from day one. C# is shipped first; TypeScript, Rust, Go, Python land as language drivers without schema changes.
- **No migration**: this is greenfield. The existing `indexed-store-and-lifecycle` proposal references this for schema; the storage layout itself doesn't change.

## Scope

**In scope:**
- Public ID format per language (cs/ts/rs/go/py).
- Schema: tables, relations, indexes, types, defaults.
- Field semantics: what each column means, what each relation expresses.
- Wire location format.
- Kind enum (closed set).
- Multi-language strategy (single tables, language column).
- Test-file marking via glob config.
- Deferred capabilities marked explicitly (data flow, exception edges, snippet blob cache, codegen-detected isomorphism).

**Out of scope:**
- DB choice (lives in `indexed-store-and-lifecycle`).
- Storage layout on disk (lives in `indexed-store-and-lifecycle`).
- MCP tool surface, response shapes, ranking/pagination (lives in future `mcp-server` proposal).
- SCIP-specific transformation logic (lives in `scip-indexing-pipeline`).
- Per-language tree-sitter grammars (lives in `scip-indexing-pipeline`).
- Snippet retrieval — agents use their own file-read tools; this model surfaces locations only.
