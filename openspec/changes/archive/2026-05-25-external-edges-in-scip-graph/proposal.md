## Why

The SCIP indexer drops every reference whose target has zero workspace definitions. In practice that means stdlib and vendored-crate symbols are excluded from the graph — `Result::unwrap`, `Vec::push`, `tempfile::TempDir::path`, etc. On the kenn-self repo, 56.8 % of all non-definition occurrences (25,919 of 45,648) fall through this hole, including 874 distinct `Result::unwrap` call sites. As a consequence `find_symbol("unwrap")` returns zero rows and `list_callers` of any stdlib method is empty, so questions like "is `unwrap` used outside tests?" cannot be answered through kenn. The `include_external` filter on MCP search tools is silently a no-op for SCIP-driven languages because no row ever carries `is_external = true` on that path — only the JSONL/C# path populates it.

## What Changes

- Drop the `def_count == 0` arm of the gate in `derive_edges_for_document`. References to symbols with zero workspace definitions now survive — the same data-flow path the JSONL/C# producer already uses for `pkg_external` symbols. Continue dropping `def_count > 1` until a follow-up measurement on larger repos justifies relaxing it (deferred — tracked separately).
- Mark drained stubs as `is_external = true` in `flush_registry_stubs`. A drained stub is by construction a symbol whose `SymbolFrame` never arrived during ingest — i.e. defined outside this workspace. This generalizes cleanly across both ingest paths (SCIP and JSONL).
- The change is unconditional — no config flag. The kenn-self measurement (taken in this session) shows +9.5 % lance footprint and +73 % post-aggregation edges; the user-visible bug fixed (`find_symbol("unwrap") → []` becomes correct) is worth the bounded cost. A follow-up may revisit if a large-repo measurement surfaces pathological growth, but the kenn-self numbers do not justify shipping a permanent config knob.
- As a side effect, `include_external` on `find_symbol` / `search_symbols` / `list_callers` becomes meaningful for SCIP-driven languages, matching the behavior already in place for C# via JSONL.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `scip-indexer`: the def-count drop in `derive_edges_for_document` no longer covers the `== 0` case; stub-drain now stamps `is_external = true` on both SCIP and JSONL paths.

## Impact

- `crates/kenn-indexer/src/edge.rs:148-151` — drop the `== 0` arm of the gate (one-line edit; `> 1` arm kept).
- `crates/kenn-indexer/src/transform_jsonl.rs:267` — `flush_registry_stubs` tags drained stubs `external = true` before push.
- Measured index volume on kenn-self: +2 021 external symbol rows (mix of Rust stdlib refs and C# package-externals that were previously tagged `external: false`), +7 189 post-aggregation edges (+73 %), +0.6 MB lance dir (+9.5 %). Workspace symbol count unchanged.
- No breaking change to MCP tool surface. `include_external` filter parameters keep their names and defaults; their effect on SCIP-language results becomes non-trivial after a reindex.
- Existing snapshots remain readable; the change affects indexing-time behavior only. A reindex is required to see external edges.
