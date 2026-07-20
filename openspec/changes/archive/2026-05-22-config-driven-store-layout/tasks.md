## 1. Layout type and config

- [x] 1.1 Add the `[layout]` section to `kenn-config` — a single `derived_root` key (relative/absolute path or the keyword `"global"`) with a default; no `committed_root` knob. Subsume the existing `[workspace] store_root` / `vectors_root` keys.
- [x] 1.2 Add the `Layout` type in `kenn-store`: `source_root` / `committed_root` / `derived_root` plus an accessor for every path (`code_vectors_dir`, `findings_dir`, `findings_vectors_dir`, `snapshots_dir`, `live_path`, `building_path`, `runs_dir`, `lock_path`, `scip_path(slug)`, …). Resolve it once from config + source root.
- [x] 1.3 Unit-test resolution: with no `[layout]` section the roots equal today's in-repo paths exactly; with `[layout]` set, every accessor reflects it.

## 2. Route kenn-store through Layout

- [x] 2.1 Rebuild `Store` on `Layout` (hold a `Layout`, derive all paths from it); remove / subsume `roots::resolve`.
- [x] 2.2 Route the findings store (`findings_dir`, `findings/vectors/`, `local/findings/`) through `Layout`.
- [x] 2.3 Generate `.gitignore` relative to `committed_root`; verify committed artifacts (`vectors/`, `findings/`) stay tracked and every derived path is ignored.

## 3. Route kenn-indexer through Layout

- [x] 3.1 Change `index_workspace` to take a `Layout` instead of a single `workspace_root`.
- [x] 3.2 Write `scip-*.scip` under `derived_root` via `Layout::scip_path(slug)` — `driver.rs:373` no longer joins it onto the `.kenn` root.

## 4. Route kenn-cli / kenn-mcp through Layout

- [x] 4.1 `kenn-cli` resolves the `Layout` once and passes it to every subcommand (`index`, `mcp`, `embed`, `status`, `rollback`, …).
- [x] 4.2 `kenn-mcp` `serve_stdio` / `run_startup_decision` take the resolved `Layout`; this removes the `store_root`-vs-`source_root` ambiguity that the earlier `serve_stdio` review flagged.

## 5. Global derived root

- [x] 5.1 Resolve `derived_root = "global"` to `${XDG_CACHE_HOME:-~/.cache}/kenn/<project-id>/`, where `<project-id>` is the xxh3-64 of the canonicalized repository root.
- [x] 5.2 Test: `"global"` resolves to an XDG-cache path unique per repo; `committed_root` stays in-repo regardless.
- [x] 5.3 Config resolution rejects a non-default `derived_root` when `staleness.git_aware_skip = false`, with a clear error that the settings are incompatible; test the rejection.

## 6. Snapshot resolution and retention by staleness key

- [x] 6.1 Extend `decide_startup_state` to scan the retained snapshots under `derived_root` and select the one whose recorded staleness key matches the workspace; reindex when none matches.
- [x] 6.2 Change snapshot GC to LRU retention — keep the `[lifecycle] gc_keep` most-recently-accessed snapshots, evict the rest; snapshot resolution touches the selected snapshot's access time. Confirm the default single-branch layout still retains the `gc_keep` most-recent, as before.
- [x] 6.3 Integration test: two branches sharing one `derived_root` (with `gc_keep` sized for both) each resolve their own matching snapshot without clobbering, and an actively-used branch's snapshot survives the other's reindex; a workspace with no matching snapshot reindexes.
- [x] 6.4 Non-git workspaces carry no staleness key: `decide_startup_state` degrades to serving `live` (so the MCP server and `embed_pending`/`reembed` work) instead of reindexing forever; `cmd_index` still always re-indexes a non-git workspace.

## 7. Verification

- [x] 7.1 Guard against regression: no store path segment (`.kenn`, `local`, `scip-`, `vectors`, `findings`) is joined outside the layout module.
- [x] 7.2 Integration test: with no `[layout]` config the on-disk layout is identical to pre-change; with a configured `derived_root` the code graph, snapshots, and `scip-*.scip` land there and nothing derived is written in-repo.
- [x] 7.3 `cargo clippy --workspace --all-targets` is clean.
