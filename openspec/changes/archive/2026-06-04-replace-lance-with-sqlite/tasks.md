## 0. Freeze the Lance baseline (do first — while lance still exists)

- [x] 0.1 Capture Lance top-k outputs for a fixed query set (identifier, blended, `search_findings`) as committed test fixtures. The SQLite ranking-parity gates (4.4, 5.3) diff against these frozen fixtures, since lance is deleted in §6.

## 1. Backend scaffolding

- [x] 1.1 Add `rusqlite` (bundled) + `sqlite-vec` to `kenn-store`; register `sqlite-vec` via `sqlite3_auto_extension(sqlite3_vec_init)` behind one narrowly-scoped `#[allow(unsafe_code, reason = "…")]`. Introduce a `sqlite` backend module (logic in named submodules, not `mod.rs`).
- [x] 1.2 Open/create the **three** snapshot databases — `graph.db`, `knowledge.db`, `findings.db` — independently publishable, readers opened `mode=ro`/`immutable=1` (design D1). `meta.json` `backend` = `"sqlite"` + `schema_version` bump; `check_backend_marker` + `check_schema_version` enforce both (design D1, D8). *(verified: `backend_marker.rs` test green.)*
- [x] 1.3 Rewrite `DbReader` / `DbWriter` in place to be SQLite-backed (no Lance/SQLite coexistence, no dispatch enum — compatibility is not required). `open_reader` / `open_writer` open SQLite; sync calls wrapped in `spawn_blocking` at the impl boundary (design D7). *(verified: `sqlite_e2e.rs` green.)*
- [x] 1.4 **Verify `vec0` KNN works on a read-only connection** (open a built `knowledge.db` with `mode=ro`/`immutable=1` and run one `vec0 MATCH`). If it needs writable temp, fall back to `mode=ro` + `PRAGMA temp_store=memory` and record the chosen open mode (design D1, T2). *(reader opens `SQLITE_OPEN_READ_ONLY`; `vec0 MATCH` runs on the read path — `hybrid_search.rs`.)*

## 2. Code graph + code search: schema + ingest

- [x] 2.1 `graph.db` tables (symbols/defs/files/edges/packages/aggregate_*/analysis_*) with today's columns + indexes on short-id / `(language,pub_id)` keys (design D2).
- [x] 2.2 `knowledge.db`: a `sqlite-vec` `vec0` table (`embedding float[768] distance_metric=cosine`) + `name_fts` (trigram) + `doc_fts` (porter/unicode61) covering symbol-doc **and** `file_docs` (path-identified) rows (design D3, D4, D5).
- [x] 2.3 `write_batch` + the inherent ingest/aggregate/finalize ops over SQLite, one transaction + prepared statements (design D9).
- [x] 2.4 Preserve the embedding lifecycle on SQLite: index writes null vectors; index-time reconciliation reuses committed sidecar vectors by fingerprint; the background `embed_pending_into` / `reembed_into` job embeds the rest, appends a sidecar segment, and **republishes `knowledge.db`** (design D5).

## 3. Reader

- [x] 3.1 Point fetches (`fetch_symbol_*`, `fetch_file_*`, `fetch_package`, docs) as indexed lookups.
- [x] 3.2 Open-time bulk `SELECT` feeding the unchanged in-memory CSR projection; all graph traversal (`list_inbound`/`outbound`/`module_files`/`find_at_location`) unchanged (design D2). *(verified: `sqlite_e2e.rs` traversal assertion green.)*
- [x] 3.3 `scan_symbols` / `scan_edges` / `scan_aggregate_*` / `scan_analysis_*` / catalog (`distinct_*`, `count_table`).

## 4. Code search + ranking parity

- [x] 4.1 `search_symbols_by_name` / `find_symbol_tiered`: FTS5 trigram candidates + exact-match boost + `(score DESC, len(name) ASC, id ASC)` (design D3).
- [x] 4.2 `search_symbols_blended` / `search_blended_hits`: fuse FTS5 name + doc + vector arms with the existing fusion policy.
- [x] 4.3 Vector arm: `sqlite-vec` `vec0 float[768]` KNN (`MATCH … ORDER BY distance`), exact brute-force over f32 dequantized from the sidecar; `int8`/`bit` `vec0` only as an optional flagged compact path (recall tradeoff, not exact) (design D5).
- [x] 4.4 **Parity gates (per-arm).** *Re-scoped — see design "Closeout note".* The overlap-vs-Lance gate is unrunnable (the baseline's corpus is kenn's own pre-refactor source, which this change moved; Lance is deleted so no re-capture). Implemented instead as a **ranking-policy property test** (`tests/search_ranking_parity.rs`): exact-match-first, trigram retrieval, name-only identifier search, doc-arm surfacing below name matches, the ≥3-char floor, deterministic order. Vector arm validated by NN sanity (`hybrid_search.rs`), not overlap. `lance_baseline.json` retained as historical reference.

## 5. Findings store

- [x] 5.1 `findings.db` rebuilt from committed `.kenn/findings/<id>.json` records; FTS5 over finding text + a `sqlite-vec` `vec0` table reconciled from the **separate** findings sidecar (`.kenn/findings/vectors/`) (design D5b).
- [x] 5.2 `search_findings` hybrid lexical+vector preserved (signature + behaviour); `stage_findings_for_publish` independent publish preserved.
- [x] 5.3 Parity. *Re-scoped (see design "Closeout note").* The §0 baseline carried no findings data; findings ranking behaviour is covered by `tests/findings.rs` (`supersede_tombstone_and_staleness` exercises `search_findings`), the vector arm by `hybrid_search.rs`.

## 6. Drop Lance

- [x] 6.1 Remove `lance*`, `datafusion*`, `arrow*`, `parquet`, `sqlparser`, `object_store` from `kenn-store` and any re-exporters; delete `db/lance`, `db/findings`'s Lance resolver, and the Lance-specific schema/reader modules.
- [x] 6.2 Confirm the subtree is gone: `cargo tree -p kenn-cli` shows no lance/datafusion/arrow crates. *(verified: 0 matches.)*

## 7. Lifecycle

- [x] 7.1 Atomic publish via the existing `live`-pointer retarget, per-store, over `graph.db` / `knowledge.db` / `findings.db` (design D1); reader registration / GC paths updated for the file layout. *(verified: `default_lifecycle.rs` green.)*
- [x] 7.2 Old Lance snapshots rejected with the standard "reindex required" message (design D8). *(verified: `backend_marker.rs` green.)*

## 8. Validation

- [x] 8.1 Port/keep the storage + search correctness tests against the SQLite backend. *(old Lance fixtures replaced by `sqlite_e2e.rs` + the `sqlite/{reader,writer}/tests.rs` + `search_ranking_parity.rs`.)*
- [ ] 8.2 Bulk-ingest throughput on a real corpus reindex (gate: not worse than ~2× Lance) (design D9). **Unrunnable as a *relative* gate — the Lance reference is deleted (see design "Validation results").** A standalone SQLite throughput number can be captured if a regression is ever suspected; left unchecked rather than silently passed.
- [x] 8.3 Re-measure the prize: dep count, build time, release binary size — recorded in design "Validation results" (62→0 lance crates; 64 MB→11 MB binary; fat-LTO link ~7 min→1m09s, with the sccache caveat noted).
- [x] 8.4 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 8.5 `just crap-ci` green for touched functions.
- [x] 8.6 `cargo fmt --all` as the final step.
- [x] 8.7 Remove the throwaway `examples/sqlite_spike.rs` + its `rusqlite` dev-dep (superseded by the real backend dep). *(verified: no spike example, no `rusqlite` dev-dep.)*

## 9. Purge stale Lance naming (closeout rename)

The engine is gone but `lance` survives in identifiers, paths, and log filters that no longer describe anything. Rename so the codebase stops lying about its backend.

- [x] 9.1 Rename `LanceCodeNodeResolver` + the `lance_resolver` module (`db/findings/`) to a backend-neutral name (`CodeGraphNodeResolver` / `graph_resolver`); update the stale "resolves against the Lance code graph" doc comments. *(also updated the re-exports in `db/mod.rs` + `lib.rs` and the usages in `sqlite/{reader/fetch,handle}.rs`.)*
- [x] 9.2 ~~Rename~~ **Remove** `run_lance_dir()` / `run_findings_lance_dir()` / `live_findings_lance_dir()` from `layout/types.rs` (+ their unit-test assertions): they had **zero callers** (`stage_findings_for_publish` is records-based and ignores `run_dir`), so they were dead lance-named code — deletion purges the name rather than renaming dead code. Dropped the vestigial `run_dir.join("lance")` from the two test helpers. Confirmed no reader resolves a literal `"lance"` segment.
- [x] 9.3 Drop the `lance=warn,lance_core=warn,…` entries from `DEFAULT_LOG_FILTER` in `kenn-cli/src/main.rs` and `kenn-server/src/main.rs` — those crates no longer exist.

## Follow-up (not in this change)

- Stale **doc-comment** mentions of "Lance" remain in `kenn-mcp`, `kenn-indexer`, `kenn-embed`, and a few `kenn-store` comments (~25 sites). Some are correct provenance ("retired Lance", "mirror the retired Lance datasets 1:1"); others describe the current backend in the present tense and are now wrong (e.g. `api/types.rs` "Lance n-gram name index", `lib.rs` "a single storage engine, Lance"). Out of scope for this change's §9 (identifiers/paths/filters); worth a focused comment sweep.
