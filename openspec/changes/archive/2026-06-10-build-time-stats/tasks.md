## 1. Schema: the `stats` table + writer surface

- [x] 1.1 Add the `stats` table (`scope, key, subset, metric, value`, PK
      `(scope, key, subset, metric)`) to `GRAPH_DDL` in
      `crates/kenn-store/src/db/sqlite/schema.rs`.
- [x] 1.2 Register `STATS` in `crate::db::names::code` + `code::ALL` so the
      schema-drift test covers it.
- [x] 1.3 Bump `STORE_SCHEMA_VERSION` (`meta.rs`, currently 2 → 3).
- [x] 1.4 Add `DbWriter::write_stats(&[StatRow])` (`handle.rs` + `SqliteWriter`)
      doing `INSERT OR REPLACE` by the row key; add `StatRow` to `api::types`.

## 2. Entity counts at finalize

- [x] 2.1 In `writer/finalize.rs`, aggregate per `(bucket, subset)` with the
      `CASE WHEN external=1 THEN 'external' WHEN test=1 THEN 'test' ELSE 'internal'`
      split: `scope='language'` × {symbols, files, defs} per language
      (defs joined to their symbol). One row per subset — NO `all` row.
- [x] 2.2 `scope='manager'` × packages per manager (subsets internal/external;
      packages have no test).
- [x] 2.3 No grand-total / `global` entity rows — a total is summed in code
      from the subset rows (task 5.1).
- [x] 2.4 Do NOT record raw `edges` (replaced by graph counters, task 3).

## 3. Graph counters during clustering

- [x] 3.1 In `crates/kenn-analyze/src/lib.rs` `build_analysis_hook`, after
      `to_records`, tally counters from the result/records.
- [x] 3.2 Write via `write_stats` with `subset='graph'`: per-language
      (`scope='language', key=<lang>`) `nodes`, `god_nodes`, `anchors`,
      `communities` (by primary-anchor language); whole-graph
      (`scope='global', key=''`) `hierarchy_depth`, `cross_anchor_communities`.
      Alongside `write_analysis_tables`.

## 4. Reader surface

- [x] 4.1 Add a sync `SqliteConnRef::stats() -> Vec<StatRow>` core over the
      pooled `&Connection` and the async dispatch (inherent on `DbReader`, like
      `find_similar_symbols`, or a `Reader` method).

## 5. MCP overview surfacing

- [x] 5.1 `get_workspace_overview` (`tools/query.rs`) makes one `stats()` call
      and reshapes (no DB aggregation): per-language subset blocks + per-language
      `subset='graph'` counters, per-manager package counts, the whole-graph
      summary (`global`/`graph` rows) when present. Scalar
      `symbol_count`/`file_count` = in-code sum of the `symbols`/`files` subset
      rows. Treat a missing row as 0.
- [x] 5.2 Update `WorkspaceInfo` (`crates/kenn-mcp/src/types.rs`) for the new
      shape; keep the scalar totals + `config_hint` empty-classification.
- [x] 5.3 Update the `mcp-offload-blocking-storage` overview regression test
      and the `agent-guide` overview reference for the new `languages` shape.

## 6. Tests

- [x] 6.1 Store test: after finalize over a snapshot with internal/test/external
      symbols in 2 languages + packages across managers, `stats` has the
      expected `(scope,key,subset,metric)` rows and no `all`/global entity rows.
- [x] 6.2 Analysis test: with clustering enabled, per-language
      `subset='graph'` rows (`god_nodes`/`communities`) and the whole-graph
      `(scope='global',subset='graph')` `hierarchy_depth` row are present; with
      it disabled, they are absent and entity counts remain.
- [x] 6.3 Reader test: `stats()` returns those rows through the pool.
- [x] 6.4 MCP test: `get_workspace_overview` carries per-language subset
      counts, per-manager packages, and the graph summary; scalars correct.

## 7. Verification

- [x] 7.1 `cargo clippy --workspace --all-targets` clean.
- [x] 7.2 `cargo test -p kenn-store`, `-p kenn-analyze`, `-p kenn-mcp` green.
- [x] 7.3 `just crap-ci` passes.
- [x] 7.4 `cargo fmt --all` as the final step.
