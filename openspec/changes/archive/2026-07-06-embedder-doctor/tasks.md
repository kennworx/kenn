## 1. `kenn doctor` probe

- [x] 1.1 Add a `Doctor` arm to the CLI `Command` enum + dispatch (mirror how
      `Status`/`Embed` are wired in `crates/kenn-cli/src/main.rs`). Done.
- [x] 1.2 `cmd_doctor.rs` (template: `cmd_embed.rs`): build a tokio rt, call
      `shared_embedder().embed_query("hello")`, time it, and report
      dim+latency+backend / `disabled` / `Backend(msg)` with the full error.
      Done (classify/report split out + unit-tested; retries on `Starting`).
- [x] 1.3 Exit codes distinguish healthy / disabled / failed. Done (Ok/Ok/Generic).

## 2. Name the active backend

- [x] 2.1 Add a minimal read-only accessor on `SharedEmbedder` returning the
      active backend kind, so doctor can name it. Done: `BackendKind` +
      `SharedEmbedder::backend_kind()` (kenn-embed); `Backend` stays `pub(crate)`.
      (in-process vs remote; daemon-vs-external-URL split not distinguished yet.)

## 3. Make degradation visible (the swallow fix)

- [ ] 3.1 At the embed-pass failure site (`crates/kenn-mcp/src/indexing/
      orchestrate.rs`), distinguish a real `EmbedError::Backend` failure from
      the clean "no model configured" path: surface a `degraded` embed state
      carrying the error rather than setting `Ready`. Done: `EmbedStage::Degraded`;
      orchestrate's `Err` branch stores it + the cause (in a new
      `ServerState.embed_error`), `Ok` clears both. Cause also persisted to a
      `<derived_root>/embed_error` marker (`kenn_store::read_embed_error`) so the
      CLI can read it; non-backend errors leave it inert.
- [x] 3.2 `get_index_status` and `kenn status` report `degraded` (with cause)
      distinctly from `ready` and `disabled`. Done: `get_index_status` emits
      state `degraded` with the cause in `IndexStatus.error`; `kenn status` prints
      `embedder: degraded — <cause>` (+ a JSON field).

## 4. Verification

- [x] 4.1 With a working embedder: `kenn doctor` prints dim + latency + backend,
      exit 0. Done: verified end-to-end (`healthy`, dim 768, backend remote, exit 0).
- [x] 4.2 With no model configured: reports `disabled` / lexical-only. Done: the
      `Ok(None)` → `disabled` path is unit-tested (`classify`).
- [x] 4.3 Forced backend error → `kenn doctor` prints the raw error, exits
      non-zero; `get_index_status` shows `degraded`, not `ready`. Done via unit
      tests: `classify`(Err→Failed)/`report`(→non-zero), `embed_stage_str`,
      `atomic_embed_stage_round_trips` (incl. `Degraded`), and the marker
      round-trip. (No live fork+Metal e2e — it's environment-specific.)
- [x] 4.4 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
      `cargo fmt --all` last. Done.
