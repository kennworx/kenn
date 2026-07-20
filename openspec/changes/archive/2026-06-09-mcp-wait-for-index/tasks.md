## 1. `wait_for_index` tool

- [x] 1.1 Extract an `IndexStatus` builder from `get_index_status`
      (`tools/lifecycle.rs`) so the status payload is produced from a
      `&LifecycleState` in one place, reusable by the new tool.
- [x] 1.2 Add `WaitForIndexArgs { timeout_ms: Option<u64> }` and a
      `WaitForIndexResponse` (the `IndexStatus` fields plus `timed_out:
      bool`) with `JsonSchema`/`Serialize` derives.
- [x] 1.3 Implement `wait_for_index` as an async handler: compute the
      deadline from `timeout_ms` (default 30 000 ms, clamp to 120 000 ms);
      loop reading the lifecycle state under the lock, drop the lock, and
      `tokio::sleep(~250ms)` until *settled* (`Ready && !reindex_in_progress`
      or `Failed`) or the deadline passes. Never hold the lock across the
      sleep.
- [x] 1.4 Export the tool + args/response from `tools/mod.rs` and register
      it as a `#[tool(description=…)]` method on `KennMcpServer`
      (`server.rs`) so `tools/list` includes it.
- [x] 1.5 Add `"wait_for_index"` to the required-names list in the
      end-to-end stdio test
      (`tests/end_to_end.rs::mcp_server_lists_all_tools_over_stdio` — it is
      a presence check, not a count). Confirm the larger list still fits a
      single `tools/list` page (the test asserts no `nextCursor`).

## 2. Guidance toward waiting

- [x] 2.1 Point the not-ready conditions at the new tool: update
      `index_unavailable_indexing` (and the reindex-in-progress hint, if
      surfaced) in `error.rs` to mention `wait_for_index` as the way to
      block until ready.
- [x] 2.2 Add a line to the narrative agent guide
      (`crates/kenn-mcp/assets/kenn-agent.md`) so an agent that finds the
      index still building knows to call `wait_for_index` rather than
      interpret an empty/early result.

## 3. Cold-start hardening

- [x] 3.1 Add a config helper "expects symbols" = any `[language.*].enabled`
      is true (reuse the language flags already read by `ConfigHint`).
- [x] 3.2 In `indexing/orchestrate.rs::run_startup_decision`, restructure
      the `Skip { live }` branch to **open → peek → decide** rather than
      calling `install_ready_or_fail` directly: open the binding, count
      symbols via `binding.reader.connect()?.count_table("symbols")`, and
      when the count is 0 **and** the config expects symbols, drop the
      binding and take the reindex path (`run_reindex_and_install`, state
      stays `Indexing`) instead of installing the empty snapshot as
      `Ready`. Otherwise install as today.
- [x] 3.3 Ensure the re-index runs at most once per cold start (no loop):
      a language-enabled-but-empty workspace and a no-config workspace both
      settle to `Ready` with the existing config-hint after the single
      decision.

## 4. Tests

- [x] 4.1 Unit tests for `wait_for_index`: returns immediately when already
      `Ready`/settled; returns immediately on `Failed`; `timed_out: true`
      path (assert `timed_out == true` and `elapsed >= timeout`, never an
      exact-ms equality — that flakes); timeout clamping.
- [x] 4.2 Test the cold-start hardening: an empty snapshot under a
      language-enabled config re-indexes (does not serve empty `Ready`); a
      no-`kenn.toml` / all-disabled workspace settles to `Ready` without a
      re-index loop.
- [x] 4.3 End-to-end: drive `wait_for_index` through `ServerState` and
      assert the settled-vs-timed-out response shape.

## 5. Verification

- [x] 5.1 `cargo clippy --workspace --all-targets` clean.
- [x] 5.2 `cargo test -p kenn-mcp` green (incl. the updated tool-list test).
- [x] 5.3 `just crap-ci` passes (split the wait loop / startup-decision
      helpers if cyclomatic complexity regresses).
- [x] 5.4 `cargo fmt --all` as the final step.
