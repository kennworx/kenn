## 1. Lance code-graph datasets — schema and write path

- [x] 1.1 Define the Arrow schemas for the code-graph Lance datasets — `symbols`, `defs`, per-kind edge data, `aggregate_*`, `analysis_*` — mirroring today's redb tables column-for-column (design D1). Edge rows are written source-sorted so a CSR builds in one pass.
- [x] 1.2 Create the scalar indexes (design D6): BTREE on the search keys (symbol-name column, `pub_id`, `short_id`, `path`); BITMAP on the low-cardinality filter columns (`kind`, `language`, `external`, `test`, edge kind). No index on edge data. A dataset below a small-corpus row count skips its indexes — a scan of so few rows is already within the planning floor.
- [x] 1.3 Implement `write_batch` over the Lance code-graph datasets — append per-batch records; resolve each name row's `sig` from in-memory build state, not a redb `SYMBOL_DOCS` point-read (design D4).
- [x] 1.4 Move key-collision dedup into the writer (design D5): the aggregation pass canonicalizes `aggregate_edges` endpoints (`min`, `max`), merges weights per `(node_min, node_max, kind)`, and emits one row per triple. Audit any other redb key-collision dedup and make it explicit.
- [x] 1.5 Guard the `short_id`-`PRIMARY` invariant: `write_batch` `debug_assert`s that no `short_id` repeats across a run's `symbols` / `files` / `packages` records — redb's key-overwrite backstop is gone (design D5).

## 2. Unified snapshot — one building dir, one atomic publish

- [x] 2.1 Build every derived Lance dataset for a run (code graph + knowledge store) into a single `building/` directory (design D2).
- [x] 2.2 `end_run` finalizes — compact fragments, build the scalar / search indexes — then publishes the whole snapshot with one atomic directory swap (`building/` → `snapshots/<ts>/`, `live` symlink flip).
- [x] 2.3 Verify a reader only ever observes a fully-built snapshot; a run failing before the swap leaves the prior `live` snapshot intact.
- [x] 2.4 Relocate the knowledge store from `.kenn/knowledge/` into the per-run snapshot under `.kenn/local/`: repoint the knowledge-path logic in `layout.rs` (`knowledge_dir()`), `db/mod.rs` (`knowledge_dir_for`, `knowledge_build_dir_for`, the `.kenn/knowledge` error strings), and `writer.rs`; drop the now-dead `knowledge/` line from `.kenn/.gitignore` (design D1/D2).

## 3. In-memory graph reader

- [x] 3.1 At reader open, bulk-scan the edge dataset once into an in-memory CSR adjacency structure (design D3).
- [x] 3.2 Serve `list_inbound` / `list_outbound` / `list_module_files` / `find_at_location` and the aggregate scans from the in-memory graph — no per-vertex query.
- [x] 3.3 Add a batched hydration helper: collect a set of `short_id`s and resolve them to records with a single `take()` call (design D3).

## 4. Rewrite the redb-shaped call sites

- [x] 4.1 `reader.rs`: replace the per-hit `read_symbol` loop in `find_symbol_tiered` / `search_symbols_by_name` with a single batched `take()` (design D4).
- [x] 4.2 `find_symbol_tiered` tiers 1–2: replace the `SYMBOLS_BY_NAME` redb range scans with a BTREE equality query (exact) and range query (prefix) on the name column (design D4, `mcp-symbol-search` spec).
- [x] 4.3 `kenn-analyze`: replace the per-vertex `edge_*` range scans with traversal over the in-memory CSR (design D4).
- [x] 4.4 Re-implement the findings `CodeNodeResolver` against the Lance code graph — `RedbCodeNodeResolver` (which probes the redb `SYMBOLS_BY_LANG_PUB_ID` table) becomes a Lance-backed resolver; `reader.rs::code_node_resolver()` returns it. The `findings-store` staleness behavior is unchanged.
- [x] 4.5 Audit every storage call site — no per-item Lance query remains inside a loop (design D3, the load-bearing constraint).

## 5. Retire the custom commit / merge machinery

- [x] 5.1 Drop the custom `CommitHandler` (ULID manifest paths) from the code-graph and search/knowledge stores — they take Lance's default manifest path. The handler itself stays for the still-committed findings store, which the separate `committed-findings` change retires (design D7).
- [x] 5.2 Remove the git-merge handler, fragment renumbering, and index-preservation-across-merge code paths from the search/knowledge store — dead once it is no longer committed. The findings store keeps its `reconcile_after_merge` (still committed; `committed-findings` retires it).

## 6. Delete redb

- [x] 6.1 Delete `db/key.rs` (the composite-key codec), `db/schema.rs` (the table definitions), `db/codec.rs`, and the redb reader/writer code paths.
- [x] 6.2 Drop the `redb` and `bincode` dependencies from `crates/kenn-store/Cargo.toml`; remove orphaned imports.

## 7. Layout version and migration

- [x] 7.1 Detect a pre-Lance snapshot structurally — the redb `db/schema.rs` that held `SCHEMA_VERSION` is deleted, so `GraphStore::open` treats a snapshot with no `symbols/` Lance dataset as outdated: it reports "code-graph store outdated — rebuilding" and rebuilds from source, never misreads (design D8).

## 8. Async surfaces, remove the ingest channel

- [x] 8.1 Remove the `run_blocking` / `spawn_blocking` wrappers around storage calls in `kenn-store`; the `Reader` implementations and the writer surface (`write_batch`, finalize) become async (design D9).
- [x] 8.2 Delete `ingest_channel.rs` and the DB-writer thread in `kenn-indexer` `pipeline.rs`; each language ingester owns an async writer and appends batches directly to the building datasets — one writer per language ingester (design D9).
- [x] 8.3 Wrap the Lance `commit` in a thin retry loop for the `Append`-vs-`Append` manifest race; move clean-stream (`Begin`/`End`) detection to the ingester tasks' join results (design D9).

## 9. Verification

- [x] 9.1 `cargo clippy --workspace --all-targets` to zero warnings.
- [x] 9.2 The existing `kenn-store` tests pass against the Lance code graph — `default_lifecycle`, `search_correctness`, `aggregate_reader`, `storage_fixtures`.
- [x] 9.3 Test: an index run produces no `.redb` file, and `redb` / `bincode` are absent from the `kenn-store` dependency tree.
- [x] 9.4 Test: `aggregate_edges` holds exactly one row per `(node_min, node_max, kind)` after symmetric edge writes — the writer-side dedup (design D5).
- [x] 9.5 Test (`kenn-store` `d3_access_pattern`): over a real 40k-symbol / 120k-edge code graph, the two D3-governed reader surfaces — the in-memory CSR built by `open_reader` and `list_outbound`'s CSR traversal + batched `take()` hydration — complete well under the per-item-loop cost. The corpus is sized so a reintroduced per-vertex scan or per-id `take()` (≈ 40k × 175 µs ≈ 7 s) trips the 2 s bound decisively — the D3 regression guard.
