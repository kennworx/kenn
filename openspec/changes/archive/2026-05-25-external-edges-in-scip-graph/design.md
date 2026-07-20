## Context

`derive_edges_for_document` in `crates/kenn-indexer/src/edge.rs` is the SCIP-path equivalent of edge extraction. It runs in two passes:

1. Pass 1 (`pipeline.rs:469-481`): walk every document and build a workspace-wide map `def_counts: HashMap<scip_symbol, usize>` counting occurrences with `Definition` role.
2. Pass 2 (`edge.rs:121-186`): for each non-definition occurrence, drop it if `def_count == 0 || def_count > 1`, otherwise emit an edge from the enclosing workspace symbol to the target.

The `> 1` arm was added to suppress noise where rust-analyzer emits a definition role from every source file for the crate-root marker (`<crate-name> 0.0.0 crate/`). The `== 0` arm was added in the same change to keep edge derivation symmetric, but it is the larger of the two effects and unintentionally hides every reference to a non-workspace symbol.

Investigation (see `tmp/scip_dropped/` diagnostic and conversation 2026-05-25):
- 56.8 % of references in `.kenn/local/scip-rust.scip` are dropped by `== 0` (25,919 occurrences across 1,479 distinct external targets).
- 2.1 % are dropped by `> 1` (955 occurrences, 21 distinct targets, all crate-root markers or duplicated test helpers — zero genuine overloads on this repo).
- 41.1 % are kept.
- The SCIP file contains zero non-workspace documents, so relaxing the gate cannot pull in external→external edges; only user→external are physically representable.

The JSONL path (`transform_jsonl.rs`, used by kenn-dotnet for C#) already populates `is_external = true` on symbols whose package is marked external (line 409-410). The SCIP path has no equivalent — `build_stub_from_scip` (`transform.rs:572`) hard-codes `external: false` and no later pass corrects it. As a consequence the `include_external` filter on MCP tools is a no-op for SCIP-driven languages.

## Goals / Non-Goals

**Goals:**
- Allow user→external edges to reach the graph on the SCIP path.
- Populate `is_external = true` on symbols that originated outside the workspace, matching the JSONL path's semantics.
- Make `include_external` filter on `find_symbol` / `search_symbols` / `list_callers` actually do something for SCIP-driven languages.

**Non-Goals:**
- Relaxing the `def_count > 1` arm. Deferred; the kenn-self evidence is workspace-specific (zero real overloads). Larger repos may show different patterns (see deferred measurement task).
- Replacing the crate-root marker filter with a kind-based filter. Deferred to the same follow-up.
- Indexing external→external edges. Not representable in current SCIP output — rust-analyzer only emits documents from inside the workspace.
- Changing the JSONL/C# path's full-symbol behavior. `pkg_external` tagging on full symbols already does the right thing; only the drain-time stub tagging changes there.
- Re-indexing existing snapshots automatically. The change affects ingest only; existing snapshots stay readable but won't include external edges until the next reindex.
- Adding a config flag to opt out. See "No config flag" below.

## Decisions

### Drain-time tagging (option A)

`flush_registry_stubs` sets `rec.external = true` on every drained stub before pushing to the sink. Rationale: a stub is buffered the first time an unknown SCIP symbol is interned. When the symbol's defining `SymbolFrame` arrives later in the same job, `mark_full_emitted` removes the stub from the pending map and the full record replaces it. Any stub that survives to drain is therefore *never-upgraded*, which is exactly the condition for "no workspace definition exists" — i.e. external.

`flush_registry_stubs` is shared by the SCIP and JSONL paths. The logic above generalizes: a stub that survives to drain on the JSONL path is also a symbol whose defining `SymbolFrame` never arrived, which on a well-behaved C# producer means external (third-party / BCL). The JSONL path also has a separate `pkg_external` plumbing on *full* symbols (`transform_jsonl.rs:409-410`); that path is unaffected and continues to set `external` from the producer's `PackageFrame`. Drain-time tagging closes the stub-only gap on both paths. Failure-mode parity holds: on a truncated stream or buggy producer, a workspace stub will mis-classify as external on either path — preferable to silent data loss.

Alternatives considered:
- *Option B — tag at stub creation, gated on `def_counts(scip_symbol) == 0`.* Provably correct against the same map the edge gate consults. Rejected on cost: requires threading `def_counts` into `intern_symbol_with_stub` and its two non-edge callers (`transform.rs:370`, `transform.rs:414`). For ~5 lines of plumbing it offers no behavioral advantage on well-formed SCIP — both options agree.
- *Per-package detection on the SCIP side, mirroring `pkg_external` from JSONL.* Rejected because rust-analyzer SCIP does not emit package records — there is no signal to populate `pkg_external` from.

Failure mode of drain-time tagging: if the SCIP file is truncated and a workspace symbol's `SymbolFrame` is missing, its cross-document stub will be drained and mis-tagged external. This surfaces as a useful diagnostic (the symbol shows up where it shouldn't) and is preferable to silent data loss.

### No config flag

The initial proposal hedged with a config flag (`[graph] include_external_edges`, default `false`) to defer the cost question. Mid-implementation we ran the measurement (see "Measured cost" below) and the bounded numbers — +9.5 % lance footprint, +73 % post-aggregation edges on kenn-self — do not justify the permanent config surface cost of a knob nobody should turn off. Specifically:

- The current behavior is a *bug* from the user's perspective (`find_symbol("unwrap") → []` and `include_external` MCP filter silently no-ops on SCIP). A flag-default-off ships that bug.
- The JSONL/C# path already accepts the equivalent cost via `pkg_external` and ships it on; SCIP being inconsistent was the original defect.
- A permanent config field that nobody wants to turn off is debt — documented in every release note, threaded through every test fixture, mentioned in every "why is my index so big?" support exchange.
- If a future large-repo measurement surfaces pathological growth, the change can be revisited with evidence; we don't need to ship a flag preemptively.

### Measured cost (kenn-self, 2026-05-25)

| Metric                  | Baseline   | With change | Delta         |
|-------------------------|------------|-------------|---------------|
| documents               | 158        | 158         | 0             |
| workspace symbols       | 4 533      | 4 533       | 0             |
| external symbols (lance)| 0          | 2 021       | +2 021        |
| edges (post-aggregation)| 9 789      | 16 978      | +7 189 (+73 %)|
| lance dir size          | 6.3 MB     | 6.9 MB      | +0.6 MB (+9.5 %)|
| `find_symbol("unwrap")` | `[]`       | 2 rows      | bug fixed     |

The 2 021 external rows are a mix of Rust stdlib refs (the primary target — `Result::unwrap`, `Vec::push`, etc.) and C# package-externals that previously sat in the lance table tagged `external: false` (now correctly `external: true`). The post-aggregation +73 % figure closes the open question the original proposal flagged about pre-dedup vs post-dedup growth — `(source, target, kind)` dedup roughly halves the pre-aggregation 140 % figure.

### Modify `scip-indexer` and `indexing-orchestrator` specs, not `source-data-model`

`source-data-model` already specifies that `is_external` exists as a `bool default false` field on symbols. The change is which symbols actually carry `true` on the SCIP path — a producer behavior, not a model definition. Likewise `mcp-symbol-search` already specifies `include_external` parameters on its tools; the change is the universe of rows those filters see, not the filter contract.

## Risks / Trade-offs

- *Index volume grows.* → Measured at +9.5 % lance dir, +73 % post-aggregation edges on kenn-self. Bounded and acceptable.
- *External symbol stubs have empty `pkg` and minimal metadata* (kind inferred from descriptor suffix, no source location, no signature). Search results will include rows with sparse info. → Mitigation: the existing `is_external` field on result rows lets clients distinguish; MCP tool callers can pass `include_external: false` if they want the old shape.
- *Drain-time tagging mis-classifies incomplete-SCIP cross-doc workspace refs as external.* → Mitigation: rare in practice; surfaces as a visible anomaly the user can act on rather than data loss.
- *The `> 1` filter remains in place and continues to drop legit test-helper edges (~166 occurrences on kenn-self).* → Accepted as deferred; better to land the `== 0` relaxation in isolation than to bundle two changes whose evidence base differs.

## Migration Plan

1. Land the code change. The change is unconditional.
2. No data migration is required. Existing snapshots stay readable; users see external edges after the next `kenn index` run.
3. A reindex on first launch after upgrade is the natural trigger for users to pick up the new behavior.

## Open Questions

- Should `def_count > 1` also be relaxed? Deferred to a follow-up; kenn-self evidence (zero genuine overloads on this repo) is workspace-specific and a separate change with its own measurement is the right place to decide.
