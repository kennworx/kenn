## 1. Graph dataset column renames (kenn-store)

- [x] 1.1 `symbols`: `short_id → id`, `pkg → pkg_id`, `enclosing_symbol → enclosing_sym_id` (graph `schema.rs` `COL_*` + `symbols_batch` + readers)
- [x] 1.2 `symbol_docs`: `symbol → sym_id`
- [x] 1.3 `edges`: `source → src_id`, `target → target_id`, `corr_canonical → corr_canon_id`
- [x] 1.4 `files`: `short_id → id` (keep `path`)
- [x] 1.5 `packages`: `short_id → id` (keep `name`)
- [x] 1.6 `aggregate_nodes`: `short_id → id`; `aggregate_edges`: `node_min → min_id`, `node_max → max_id`
- [x] 1.7 Leave the derived-analysis datasets untouched (out of scope)

## 2. Search dataset column renames (kenn-store lance)

- [x] 2.1 `short_id → id` (volatile join key)
- [x] 2.2 `id` / `stable_id` → `embed_key` (internal composite embedding-reconciliation key)
- [x] 2.3 `pub_id` stays `pub_id` (symbol's API-visible public id) — not renamed
- [x] 2.4 Update `build_batch_rows`, reconciliation/reuse read path, and reader hydration to the new names

## 3. Model + propagation

- [x] 3.1 Rename `kenn-model` record fields to match (`SymbolRecord.short_id → id`, `.pkg → pkg_id`, `FileRecord.short_id → id`, etc.) and fix all referencing crates; `SymbolRecord.pub_id` is **unchanged**. Also extended to the read-side `*Row` DTOs (api/types.rs) + kenn-mcp consumers per the agreed scope (option 2).
- [x] 3.2 Confirmed: JSONL wire `Frame` field names (parse_jsonl) and MCP response JSON are unchanged — `*Row` DTOs don't derive `Serialize`, so renaming their fields doesn't touch the wire. Only store columns + model/DTO fields changed.
- [x] 3.3 No `STORE_SCHEMA_VERSION` bump (no users) — schema changes in place; stale snapshot dropped + reindexed

## 4. Drop SCHEMA_CHANGELOG (keep the version-check machinery)

- [x] 4.1 Delete `crates/kenn-store/SCHEMA_CHANGELOG.md`
- [x] 4.2 Strip the `(see SCHEMA_CHANGELOG.md)` / changelog references from the error string in `crates/kenn-store/src/api/types.rs`, the doc comment in `crates/kenn-store/src/lib.rs`, the comment in `crates/kenn-mcp/src/indexing.rs`, and the status message in `crates/kenn-cli/src/cmd_status.rs` — keep the `SchemaMismatch` error, the `STORE_SCHEMA_VERSION` constant, and the strict check intact (reserved for future use)
- [x] 4.3 The mismatch error string becomes `"schema v{persisted}, binary expects v{expected}; reindex required"` (no changelog pointer)

## 5. Validation

- [x] 5.1 `cargo clippy --workspace --all-targets` clean (0 errors, 0 warnings); all affected-crate tests pass (34 suites ok, incl. regenerated `wire_format` record snapshots)
- [x] 5.2 `just crap-ci` green (PASSED: no regressions, no new over-threshold — pure renames don't change cyclomatic complexity)
- [ ] 5.3 Reindex (after MCP reload); confirm symbol search + get_symbol still work end-to-end — store dropped; **needs user to reload the rebuilt MCP binary**. (Round-trip already validated in-process: storage_fixtures read/write/search + mcp navigation/symbol_search tests all pass under the new columns.)
- [x] 5.4 `cargo fmt --all` (touched only rename-affected files + regenerated record snapshots)
- [x] 5.5 Rebase `add-file-level-docs` onto the final names — done in its spec (`file_docs.file_id`; file-doc search row: `id`=file id, `pub_id` empty, `embed_key="filedoc:<lang>:<path>"`)
