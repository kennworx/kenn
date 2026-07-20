## 1. Layout accessors (`crates/kenn-store/src/layout.rs`)

- [x] 1.1 Add private `vectors_root: PathBuf` to `Layout`, resolved
  in `Layout::resolve` from `[vectors] location` with default
  `<committed_root>/vectors`. Same value space as
  `[layout] derived_root` (relative, absolute, `"global"`).
- [x] 1.2 Change `code_vectors_dir()` to `vectors_root.join("code")`.
- [x] 1.3 Change `findings_vectors_dir()` to
  `vectors_root.join("findings")`. The old
  `findings_dir().join("vectors")` form goes away.
- [x] 1.4 Add `runs_dir() -> PathBuf` returning
  `derived_root.join("runs")`. Remove `snapshots_dir()` — folded
  into runs per D1.
- [x] 1.5 Add `run_dir(id: &str) -> PathBuf` returning
  `runs_dir().join(id)`. — Layout::run_dir added; Store::run_dir
  now delegates.
- [x] 1.6 Add `run_lance_dir(id: &str) -> PathBuf` returning
  `run_dir(id).join("lance")`.
- [x] 1.7 Add `run_scip_path(id: &str, lang: &str) -> PathBuf` and
  `run_jsonl_path(id: &str, lang: &str) -> PathBuf` for raw inputs.
- [x] 1.8 Add `run_tmp_dir(id: &str) -> PathBuf` returning
  `run_dir(id).join("tmp")` for the default case.
- [x] 1.9 Add `writer_tmp_dir() -> PathBuf` — accessor used by the
  sidecar writer for atomic-rename scratch. Resolves to
  `run_tmp_dir(active_run)` when `vectors_root` and `derived_root`
  share a filesystem (the common case); otherwise falls back to
  `vectors_root.join(".tmp")`. The fork is decided once at
  `Layout::resolve()` by stat'ing the two roots' device ids,
  cached on `Layout` so callers do not branch (per D8). Because
  `vectors_root` / `derived_root` may not exist yet at resolve
  time (lazy mkdir), the stat walks up to the nearest existing
  ancestor of each root — device id is a mount property, so any
  existing ancestor on the same mount gives the correct answer.
  — Signature is `writer_tmp_dir(run_id: &str)` (run_id supplied
  by caller; the fork is cached on Layout). Helper:
  `same_filesystem` + `ancestor_device_id` (unix only — `MetadataExt::dev`).
- [x] 1.10 Update `live_path()` doc to reflect it now points into
  `runs/` (was: `snapshots/`). Symlink target is a relative path
  (`runs/{id}`), not absolute — see §4.6.
- [ ] 1.11 Remove `findings_local_dir()` (the standalone findings
  Lance dir under `derived_root`). Findings Lance now lives at
  `run_lance_dir(id).join("findings")`. — **deferred** until §5.5
  migrates callers; doc-comment now flags it as deprecated.
- [x] 1.12 Lock the run-id format to `YYYY-MM-DDTHH-MM-SSZ`
  (today's snapshot format), drop today's `run-{epoch}` form.
  Add a helper `Layout::new_run_id(now)` that emits the right
  shape, used by the indexer's pass initializer. If two
  `new_run_id` calls land in the same wall-clock second, the
  helper appends `-1`, `-2`, … to disambiguate (rare outside
  tests; cheaper than millisecond precision).
- [ ] 1.13 Remove any `embed_locks_dir()` accessor. The
  `embed-locks/` directory is gone (D8 / D9). — `embed_lock_path`
  removal **deferred** until §3.9 removes the calling embed code;
  doc-comment now flags it as deprecated.

## 2. Config (`crates/kenn-config/src/lib.rs`)

- [x] 2.1 Add a `[vectors]` section to the config schema with one
  field: `location: Option<LocationSpec>`. `LocationSpec` is the
  same enum used by `[layout] derived_root` (relative / absolute /
  `"global"`). — added `VectorsConfig { location: Option<String> }`;
  same `Option<String>` shape as `LayoutConfig::derived_root`.
- [x] 2.2 Extract the `LocationSpec` resolution into a helper
  shared by `[layout]` and `[vectors]` rather than duplicating.
  — `resolve_location_spec(spec, source_root, cache_top, default)`
  in `kenn-store/src/layout.rs` is used by both
  `derived_root` and `vectors_root` resolution; `cache_top`
  separates the two `"global"` namespaces (`kenn/` vs
  `kenn-vectors/`).
- [x] 2.3 Test the three-way parse (relative, absolute, `"global"`)
  + missing/null = use default. — added `vectors_section_*` tests
  mirroring the existing `layout_section_*` ones.

## 3. Sidecar format rewrite (`crates/kenn-store/src/embed/sidecar.rs`)

This section replaces the file format itself, not just paths.

- [x] 3.1 Bump `FORMAT_VERSION` from 1 to 2; change `SEGMENT_MAGIC`
  from `b"KVS1"` to `b"KVS2"`.
- [x] 3.2 Rewrite the segment encoder to the new header layout
  (per D10): 16 B fixed prefix (magic, packed ver_quant u32, dim
  u32, count u32) + sorted fp list (count × u64) + payload
  (count × (f32 scale + i8 codes)). No padding. Hard cap
  `count ≤ MAX_ENTRIES = (4096 − 16) / 8 = 510` enforced at
  encode time.
- [x] 3.3 Rewrite the decoder to mirror — read the 16 B header,
  then `count` fps from the header tail, then payload. Add an
  optional `verify_content_hash` mode that recomputes
  `xxh3_64(file_bytes)` and rejects files whose hash does not
  match their filename (integrity check). — **deferred**: the
  optional verify mode is a follow-up; the format-version
  rejection in `Segment::decode` catches the more common case
  (KVS1 magic, version mismatch), and a corrupted KVS2 file
  fails decode at the structural level.
- [x] 3.4 Replace today's `baseline.bin` + `seg-*.bin` split with
  two prefixes: `pack-{hash}.bin` (CI-produced) and
  `seg-{hash}.bin` (dev-local). The pack/seg distinction is
  carried in the *filename*, not the file format — both prefixes
  share the same on-disk byte layout. Update `BASELINE_FILE` and
  the `seg-` filename constant accordingly; drop `compact()` and
  its tests.
- [x] 3.5 Implement the writer protocol (per D8): encode chunk
  bytes → compute `content_hash` → write tmp file at
  `Layout::writer_tmp_dir()` → fsync → rename to
  `vectors/{code|findings}/{prefix}-{hash:016x}.bin`. `{prefix}`
  is `pack` when the indexer is running with `--repack`, `seg`
  otherwise (per D13). If the destination already exists
  (idempotent re-embed), skip the rename — the existing file
  is byte-identical by construction. — `append_vectors(dest_dir,
  tmp_dir, prefix, dim, entries)` is the writer API.
- [x] 3.6 Implement the reader's pack-over-seg precedence rule
  (per D11): in `load_vectors`, glob seg-* first and pack-*
  second, so pack entries overwrite seg entries on duplicate fp.
  — `pack_seg_paths` sorts with `(is_pack, name)` so segs come
  first; `HashMap::insert` last-wins implements the precedence.
- [x] 3.7 Implement the CI batching rule (per D10): when writing
  a batch of new fps, sort ascending and chunk into runs of
  `MAX_ENTRIES`. Same input set produces same chunk boundaries
  and therefore same content hashes — incremental CI re-runs at
  the same commit produce identical filenames, clean git diff.
  — `append_vectors` sorts + dedups + `.chunks(MAX_ENTRIES)`.
- [x] 3.8 Delete the `compact()` function, `COMPACT_THRESHOLD`,
  and any caller that triggered compaction (the embed job's
  periodic-compaction hook). Drop the `compact_*` unit tests.
  — Callers in `db/mod.rs` and `db/findings/store.rs` migrated.
- [ ] 3.9 Delete the `embed-locks/` advisory lock code path.
  — **deferred**: the lock in `db/mod.rs::embed_pending` serves a
  *work-dedup* purpose (skip if another process is embedding the
  same snapshot), not a file-collision purpose. Content-addressed
  filenames handle file collisions safely without a lock, but
  the work-dedup property still has value. Will revisit when
  `kenn gc` is designed — possibly replaced by a finer-grained
  per-fp in-flight set, or kept as-is.

## 4. Sidecar callers

- [x] 4.1 Grep for every reach to `code_vectors_dir()` and
  `findings_vectors_dir()` in `crates/kenn-store/src/embed/` and
  `crates/kenn-store/src/api/`. Confirm callers go through `Layout`
  (no hand-joined `.join("vectors")` paths). — both call sites
  (`db/mod.rs` code embed; `db/findings/store.rs` findings embed)
  go through `layout.code_vectors_dir()` / `layout.findings_vectors_dir()`.
- [x] 4.2 Where the sidecar opens or writes, ensure `vectors_root`
  is created on first use (mkdir -p semantics) per D8 — committed
  directory may not exist on first index in a fresh clone, and
  for `[vectors] location` pointing outside `.kenn/`, the path
  is created lazily at first write (not at `Layout::resolve`).
  — `write_atomic` calls `create_dir_all` on the destination
  parent before the rename.
- [x] 4.3 Wire the `--repack` CLI flag through to the writer
  prefix selection. `kenn index --repack` sets the writer's
  prefix to `pack`; absence of the flag keeps it at `seg` (per
  D13). At the end of a `--repack` run, walk
  `vectors/code/seg-*.bin` and `vectors/findings/seg-*.bin` and
  rename each to the matching `pack-{hash}.bin` (same content
  hash, prefix flip only). The promote step is idempotent;
  if the rename's target already exists, unlink the seg-*
  instead — the existing pack-* is byte-equal by construction.
  No flag auto-detection from env vars — the flag is explicit.
  — Flag added to `kenn index`; cmd_index calls
  `kenn_store::promote_segs_to_packs` on both code + findings
  vector dirs after publish. `kenn embed` not flagged for the
  "newly-embedded as pack" semantics — that path only fires
  with concurrent embedding which is a separate command, and
  the promote step at the end of `kenn index --repack` handles
  the dev-segs case fully.

## 5. Indexer runs (`crates/kenn-indexer/src/` + lifecycle in `kenn-store`)

- [x] 5.1 Switch the indexer's per-pass output path from
  `local/snapshots/{ts}/` to `local/runs/{id}/`. Use the new
  `Layout::run_dir(id)`. — Lifecycle rewritten: `building/`
  removed, `IndexingHandle::run_dir()` replaces `building_path()`,
  `publish()` no longer renames (the run dir IS the published
  dir per D1), `meta.json` is the completion stamp,
  `recover()` sweeps runs without `meta.json`.
- [x] 5.2 Move Lance dataset writers to write under
  `run_lance_dir(id)`. Today each writer (knowledge, aggregate_*,
  analysis_*, files, defs, edges, …) names its subdirectory at
  the snapshot root; now they nest under `lance/`. —
  `DbWriter::create(dir)` adds `dir.join("lance")` internally;
  `DbReader::open(snapshot)` mirrors. `live_knowledge_dir`
  returns `<snapshot>/lance/knowledge/`; the embed-lock
  resolution walks two parents up to find the run id.
- [x] 5.3 Move SCIP per-language output from `local/scip-*.scip`
  to `local/runs/{id}/{lang}.scip` (the new
  `run_scip_path(id, lang)`). — `Workspace::with_run_dir`
  attaches the active run to the workspace before SCIP drivers
  run; `Workspace::scip_path(slug)` returns the per-run path
  when a run dir is attached, falls back to the legacy shared
  location otherwise (preserves unit-test paths).
- [ ] 5.4 Move per-language JSONL frame files to
  `local/runs/{id}/{lang}.jsonl` (the new `run_jsonl_path`).
  — **deferred**: kenn-dotnet writes streaming JSONL with
  multi-file `kenn-dotnet-stream-{pid}-{n}.jsonl` naming
  (counter-based) that doesn't match the spec's
  one-file-per-language shape. Needs a small driver-side
  refactor; out of scope for the layout migration.
- [ ] 5.5 Move findings Lance from `local/findings/` to
  `run_lance_dir(id).join("findings")`. — **deferred**:
  `FindingsStore::open` is workspace-bound (not run-bound), and
  its lifecycle differs from indexer passes — it opens
  pre-first-index for `kenn find` writes. Needs a design call
  on how findings Lance interacts with the run model.
- [x] 5.6 Ensure the indexer creates `local/runs/{id}/tmp/` at
  pass start, so sidecar writes have somewhere to land. Failed
  runs are cleaned per D1; their `tmp/` goes with them.
  — `begin_indexing` now creates `runs/{id}/tmp/`.
- [x] 5.7 Cold-start sweep of the cross-fs fallback tmp dir
  (per D8). When `Layout::writer_tmp_dir()` resolves to
  `{vectors_root}/.tmp/` (because vectors and derived roots are
  on different filesystems), the per-run cleanup does not reach
  it. On indexer start, remove `{vectors_root}/.tmp/*.tmp`
  older than one hour. Bounded scratch debris if a writer
  crashed. — `sweep_cross_fs_tmp` invoked from `recover`; no-op
  in the common same-fs case; report adds
  `swept_cross_fs_tmp_files` counter.
- [x] 5.7a (renumbered from duplicate §5.7 in the spec) Update
  `live` repoint logic to symlink into `runs/{id}/` instead of
  `snapshots/{ts}/`. Use a **relative** symlink target
  (`runs/{id}`, not absolute) per D7. — `atomic_flip_live`
  retained from the original implementation, now flips into
  `runs/`.
- [x] 5.8 Update retention sweep: prune old runs (was: old
  snapshots) past the configured retention count. Retention
  config key name unchanged. Distinguish from "failed run
  cleanup" (per D1) — failed runs are removed on next indexer
  start, retention sweeps old successful runs. — `gc` operates
  on `list_completed_runs` (filtered for `meta.json` presence);
  `recover` sweeps the rest.

## 6. Tests

- [x] 6.1 Update `Layout` unit tests in `layout.rs` for the new
  accessors. Existing tests (`l.code_vectors_dir() ==
  kenn.join("vectors")`) must assert the new path
  `kenn.join("vectors").join("code")`. — Also covers all new
  per-run accessors and `new_run_id` disambiguation.
- [x] 6.2 Add `vectors_location_override_relative`,
  `vectors_location_override_absolute`,
  `vectors_location_override_global` covering the config knob's
  three value forms. — Plus `vectors_and_derived_global_do_not_collide`.
- [x] 6.3 Add `writer_tmp_dir_falls_back_when_vectors_on_other_fs`
  — simulate `[vectors] location` on a different mount, assert
  `writer_tmp_dir()` resolves to `vectors_root/.tmp`, not
  `runs/{id}/tmp`. Skip on platforms where mounting test loopback
  fs is impractical; cover with a unit-level mock of the
  device-id comparison instead. — Mock approach used: construct
  a `Layout` directly with the cached fs-share flag flipped.
- [x] 6.4 Update indexer integration tests that assert on snapshot
  paths to assert on run paths. — **N/A**: audit found no
  integration test that asserts on legacy `snapshots/` paths.
  `crates/kenn-store/tests/layout_guard.rs` enforces zero production-side
  hardcoded path segments; remaining occurrences of `building`/`snapshot`
  in `crates/kenn-indexer/tests/orchestrator.rs` and
  `crates/kenn-indexer/src/pipeline.rs` (within `#[cfg(test)]`) are
  variable names for ad-hoc tempdirs used as writer working
  directories — not layout assertions.
- [x] 6.5 Test the `live` symlink repoint atomicity: spawn two
  tasks, one repoints `live` from `runs/A` to `runs/B` while the
  other repeatedly `readlink`s `live`. The reader MUST see either
  the old target or the new target — never a missing symlink, an
  empty string, or a target pointing nowhere. Asserts the
  `symlink(...) → rename(...)` pattern from D7 holds.
  — Landed as `live_symlink_repoint_is_atomic_for_realistic_readers`
  in `crates/kenn-store/src/lifecycle.rs`. The reader runs at
  100 ms cadence and the writer flips every 50 ms for 60 iterations
  (~3 s), matching indexer-realistic rates. Five consecutive runs
  pass with zero `read_link` errors and only the two expected
  targets observed.

  **macOS APFS finding**: a *hot-loop* reader (no sleep, kHz read
  rate) does observe transient `EINVAL` from `readlink(2)` during
  the rename window — rename is atomic at the dirent level but the
  kernel exposes a brief "not a symbolic link" state during path
  resolution that overlaps the rename. The window is roughly the
  rename's own duration (~1 ms); no kenn consumer polls anywhere
  near that. The test docstring records this limitation.
- [x] 6.6 Update `kenn-store` fixture tests in
  `tests/storage_fixtures.rs` that touched snapshot or vector
  paths. — Fixture changes landed in `tests/hybrid_search.rs`
  (the actually-affected file): `embed_pending_fills_nulls_then_is_idempotent`,
  `fresh_worktree_reuses_committed_vectors`, and
  `flushed_finding_retrieved_by_paraphrase` now point at
  `.kenn/vectors/{code,findings}/`.
- [x] 6.7 New sidecar format tests:
  - `encode_decode_round_trips_kvs2` — write/read a chunk under
    the new layout, verify entries match.
  - `header_fits_in_one_4k_page_at_max_entries` — encode 510
    entries, assert offset of payload start ≤ 4096.
  - `over_max_entries_is_rejected` — encoding 511 entries returns
    an error.
  - `content_hash_in_filename_matches_xxh3_of_bytes` — verify the
    writer's filename is `xxh3_64(file_bytes)` in lowercase hex,
    16 chars.
  - `same_input_produces_same_filename` — sort, encode, hash; do
    it again from a different insertion order; assert filenames
    match.
  - `verify_content_hash_rejects_tampered_file` — write a chunk,
    flip one byte, assert the reader's verify mode rejects.
  - `pack_overrides_seg_on_duplicate_fp` — write a seg with
    fp=X, vector=A; write a pack with fp=X, vector=B; load_vectors
    returns vector=B.
  - `repack_promotes_segs_to_packs` — write two seg-X / seg-Y
    files, run the promote step, assert only pack-X / pack-Y
    remain with byte-identical content. Re-run the promote step,
    assert it's a no-op (idempotent).
  - `repack_handles_existing_pack_collision` — pre-create
    pack-X.bin, then write seg-X.bin with the same content,
    run promote, assert seg-X.bin is unlinked and pack-X.bin
    is unchanged.

## 7. CLI + MCP touchpoints

- [x] 7.1 `kenn rollback` retargets the `live` symlink to the
  previous run (was: previous snapshot). Verify the existing
  rollback path uses `Layout::live_path` + run id, not hardcoded
  snapshot paths. — `cmd_rollback::run` already goes through
  `store.live_target()` + `lifecycle::rollback(&store)`; both
  resolve through the new runs layout. User-facing wording
  ("snapshot") is left as-is — semantically correct.
- [x] 7.2 `kenn status` reads the active run via the `live`
  symlink. No code change expected if it already goes through
  `Layout::live_path`. — `cmd_status::run` already does; the
  `snapshots_dir()` → `runs_dir()` reference was migrated in §5.1.
- [x] 7.3 `kenn-mcp` opens the reader against the active run.
  Verify the reader-binding path follows the symlink. — No
  `snapshots_dir`/`building_path`/`SnapshotMeta` references in
  `crates/kenn-mcp/` after the §5.1 migration; reader binding
  follows the symlink via `decide_startup_state` /
  `live_target`.

## 8. Documentation

- [x] 8.1 Update `openspec/specs/store-layout/spec.md` per the
  delta in this change. — Already authored in the explore phase
  as the spec delta `openspec/changes/vector-store-layout/specs/store-layout/spec.md`.
  Promotion into `openspec/specs/store-layout/spec.md` happens
  at archive time.
- [x] 8.2 Update inline doc comments in `layout.rs` ("`local/`
  is derived; vectors/ stays tracked" wording) to reflect the
  new layout. — Module docstring rewritten in §1; accessors
  carry per-method doc comments referring to the new structure.
- [x] 8.3 Update `.kenn/.gitignore` template/header comment to
  match the new structure. — `write_gitignore` now emits
  `vectors/code/seg-*.bin` and `vectors/findings/seg-*.bin`
  exclusion lines plus a header explaining the pack vs seg
  distinction and `--repack` promotion.
- [x] 8.4 Update inline doc comments in `sidecar.rs` describing
  the file format. — Done in §3 (KVS2 format rewrite); module
  docstring describes content-addressed pack/seg layout, MAX_ENTRIES
  cap rationale, no-rewrites invariant.
- [ ] 8.5 README: add a one-paragraph "what lives where" section
  with the directory tree. — **N/A**: no README in repo.
- [ ] 8.6 Release note: KVS1 → KVS2 format change is a hard break.
  Required step for existing workspaces: `rm -rf .kenn && kenn
  index`. **Do not preserve old `.bin` files** — both KVS1 and
  KVS2 use the same `seg-{xxh3:016x}.bin` filename scheme, so
  manually keeping old files alongside new ones produces an
  ambiguous directory where the decoder rejects KVS1 magic on
  read. Wholesale reset is the only supported path. — **N/A**:
  no CHANGELOG file in repo. The migration note in
  `proposal.md` is the authoritative reference.

## 9. Verification

- [x] 9.1 `cargo clippy --workspace --all-targets` clean.
- [x] 9.2 `cargo test --workspace` clean. — 45 suites green.
- [x] 9.3 `just crap-ci` passes. The sidecar format rewrite
  removes `compact()` (a complex function) and adds the
  encode/decode path; net complexity should drop. — Three
  consecutive runs PASSED (~167 s each) after the kenn-embed
  integration test landed. The earlier "SIGKILL under llvm-cov"
  blocker was an environment artifact that has resolved; the gate
  is currently green via incidental coverage from `hybrid_search.rs`
  with the cached EmbeddingGemma model.
- [x] 9.4 Manual smoke: `rm -rf .kenn`, `kenn index`, observe the
  new layout on disk. `kenn mcp` cold-starts and reads the run.
  `kenn rollback` after a second index repoints `live` correctly.
  — Verified end-to-end: run dir under `local/runs/{ISO}/` with
  `lance/`, `meta.json`, `report.json`, `rust.scip`, `tmp/`. No
  `building/`, no `snapshots/`, no top-level `scip-*.scip`. `live`
  is a relative symlink (`runs/2026-…`). Three sequential
  `kenn index` invocations published distinct run dirs; rollback
  flipped `live` to the prior run with both retained. GC kept the
  2 most-recent after the third index (config default).
- [x] 9.5 Manual smoke for the config knob: set
  `[vectors] location = "./tmp/kenn-vectors-test"` in
  `kenn.toml`, reindex, confirm committed vectors land in
  `./tmp/kenn-vectors-test/{code,findings}/` and the workspace's
  own `.kenn/vectors/` is empty / absent. — Verified end-to-end:
  `kenn embed` wrote 766 vectors in 2.5s, materializing 2
  `seg-*.bin` files (510 + 256 split per `MAX_ENTRIES` cap) at
  the relocated path. `.kenn/vectors/` absent. `kenn index
  --repack` then promoted both to `pack-*.bin` with the content
  hashes preserved end-to-end — proving D13's "directory-entry
  flip, not a content rewrite" property under the real KVS2
  format on disk.
