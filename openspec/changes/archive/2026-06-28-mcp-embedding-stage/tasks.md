## 1. Store — distinguish disabled from done

- [x] 1.1 `ReembedReport` gains `embedder_available: bool`
  (`crates/kenn-store/src/db/jobs.rs`): `false` only in the no-embedder branch,
  `true` otherwise (including nothing-pending). → verify: compiles; disabled branch
  sets false.

## 2. MCP — track the embed stage

- [x] 2.1 Add `EmbedStage` (Building/Ready/Disabled, snake_case serde) + an
  `AtomicEmbedStage` cell in `crate::state`, mirroring `WatcherState` /
  `AtomicWatcherState`. → verify: serde round-trips snake_case.
- [x] 2.2 `ServerState` gains `embed_stage: Arc<AtomicEmbedStage>` (default Ready).
  `spawn_embed_job` takes the handle: store `Building` on spawn; on completion store
  `Ready` (embedder_available) or `Disabled` (not); `Err` → `Ready` (logged). Both
  call sites pass `state.embed_stage.clone()`. → verify: compiles.

## 3. MCP — report the stage

- [x] 3.1 `build_index_status` takes the embed stage and, when the lifecycle is
  `Ready`, reports `state` as `embedding` (Building) / `ready` (Ready) / `disabled`
  (Disabled). `get_index_status` and `wait_for_index` pass
  `state.embed_stage.load()`. → verify: status shows `embedding` while a pass runs.

## 4. MCP — find_similar transient vs terminal

- [x] 4.1 `find_similar`: when a symbol has no committed vector, read the embed
  stage — `Building` → transient retryable error (embeddings still building);
  `Ready`/`Disabled` → terminal `EMBEDDING_UNAVAILABLE`. → verify: behavior matches
  the stage.

## 5. Docs + spec

- [x] 5.1 Update the `get_index_status` tool description + `IndexStatus` doc to list
  the five `state` values and the structural-from-`embedding` contract; note it in
  the kenn skill. → verify: docs list all five.
- [x] 5.2 Spec deltas: `mcp-server` (state values + contract), `mcp-symbol-search`
  (transient-vs-terminal). → verify: `openspec validate`.

## 6. Gates

- [x] 6.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
  `cargo fmt --all` last.
