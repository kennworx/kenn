# Tasks

## 1. Tool implementation (`kenn-mcp`)

- [x] 1.1 Add the `find_usages` tool: args `query` (required) + optional `kind`/`path`/`package`/`language` (reuse the existing `Filters`), `edge_kinds`, `include_external`, `page_size`/`cursor`. → verify: schema compiles; `tools/list` includes `find_usages`.
- [x] 1.2 Resolution dispatched by query form: `pub_id` → direct; workspace-relative path → file lookup (`fetch_file_short_id`), or attachment stub for a non-indexed asset; plain name → `find_symbol` name index. Apply narrowing filters. → verify: name, path-to-file, asset-path, and `pub_id` inputs each resolve via the right route in unit tests (a path does NOT go through the name index).
- [x] 1.3 Traversal: for each resolved target, gather incoming edges (the `list_usages` path) with the default reference-style edge set (`calls`,`type_use`,`field_access`,`instantiates`,`imports`,`links_to`,`links_to_file`,`embeds`,`uses_css_class`), overridable by `edge_kinds`. → verify: default includes `imports` (a stylesheet's `<link>` importers surface); explicit `edge_kinds` narrows.
- [x] 1.4 Response: a single **flat list of references, each row tagged with its resolved target** (uniform shape); cap distinct targets (top-N) and report truncation. Single target → paginate (cursor); multiple → `next: null`. **The tool description MUST state the narrow-to-paginate rule** (user requirement). → verify: ambiguous query → flat tagged list, `next:null`, truncation reported; single target → `next` cursor present.

## 2. Contract conformance (`mcp-server`)

- [x] 2.1 Register `find_usages` in the paginated-tool list, the empty-snapshot list, and the **search-tool exemption** list — NOT the unresolved-entity-error list (it is query-shaped: empty is a valid answer). → verify: a query resolving to nothing AND a real-but-unreferenced asset (`assets/unused.png`) both return **empty, not an error**; empty-snapshot still errors.
- [x] 2.2 Cursor pagination for the **single-target** case (edge-kind-ordinal + last-short-id over the fixed target); multi-target sets `next:null`. → verify: single-target `next` cursor round-trips; stale cursor → `-32602`; multi-target returns `next:null`.

## 3. Docs & verification

- [x] 3.1 Document `find_usages` in the `kenn` skill as the one-call "where used" path; keep `find_symbol`+`list_usages` as the stepwise/power alternative. → verify: skill names the tool and its page_size envelope.
- [x] 3.2 End-to-end: `find_usages` over a fixture with a symbol, a file, and an attachment stub (e.g. an `<img>`-referenced asset). → verify: one call returns references for each; `cargo clippy --workspace --all-targets` clean; `just crap-ci` passes.
