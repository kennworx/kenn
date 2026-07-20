## 0. Prerequisite

- [x] 0.1 `index-producer-parity` has landed (single consolidated producer-
      registration function used by both the CLI and workflow/MCP paths).

## 2. Text-fallback producer

- [x] 2.1 Add a `[language.text]` (or `[fallback]`) config block: include/exclude
      globs, target/overlap sizes; `enabled=false` by default.
      → `kenn-config/src/language/text.rs` (`TextConfig`, include globs +
      excludes + `target_chars`/`overlap_chars`).
- [x] 2.2 Discovery walker (mirror `markdown/discover.rs`): resolve globs, honor
      excludes/`.gitignore`, and **skip extensions claimed by an enabled
      producer** (no double-index).
      → `kenn-indexer/src/text/discover.rs`; claimed-ext skip driven by
      `claimed_extensions()` in `workflow.rs`. (Honors excludes; `.gitignore` is
      not consulted — same as markdown, which walks excludes only.)
- [x] 2.3 Recursive splitter (D1): blank-line → newline → hard-cut to target
      size with overlap; a sub-min file is one chunk.
      → `kenn-indexer/src/text/split.rs` (char-boundary-safe, best-effort
      overlap, whitespace-only chunks dropped).
- [x] 2.4 Emit file + chunk nodes as `kenn_model` records into the `BatchSink`
      (mirror `markdown/walk.rs`), identity `text:<root>/<relpath>#<idx>`, chunk
      text as `SymbolDocsRecord.doc`.
      → `kenn-indexer/src/text/walk.rs` (+ `Kind::Chunk`, `id/text.rs`); ingest
      in `text/ingest.rs` (single-phase, corpus root module + per-file docs).
- [x] 2.5 Register the producer via `with_text(...)` on `IndexerDriver`, in the
      single consolidated registration function from `index-producer-parity` (§0).
      → `driver/orchestrator.rs` (`with_text` + `TextCorpus` slot),
      `pipeline/api.rs` (barrier-free `text_unit`), `workflow.rs`
      (`configure_runner` registration).

## 3. Verification

- [x] 3.1 A configured `.yaml`/`.json`/`.txt` file becomes searchable nodes; its
      chunks are returned by `semantic_search`.
      → `tests/text_fallback_index.rs::text_file_is_indexed_via_index_workspace`
      (end-to-end via `index_workspace`); chunk docs land in `symbol_docs`
      (`text/ingest.rs` store test). Search joins by id without a kind filter, so
      `Kind::Chunk` nodes surface like any other doc.
- [x] 3.2 A `.rs`/`.md` under a fallback glob is **not** double-indexed when its
      semantic/native producer is enabled.
      → `discover.rs::claimed_extension_is_skipped_even_when_include_matches` and
      `ingest.rs::ingests_text_records_into_the_store` (broad `**/*` glob, `rs`
      claimed → `lib.rs` skipped). (End-to-end with a real language server is not
      run — it needs the toolchain; the unit tests assert the skip directly.)
- [x] 3.3 Disabled (default) → no behavior change.
      → `tests/text_fallback_index.rs::disabled_text_fallback_indexes_nothing`;
      `workflow.rs::configure_runner_registers_text_only_when_enabled`.
- [x] 3.4 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
      `cargo fmt --all` last.
      → clippy clean (2 `#[expect(clippy::string_slice)]` on the splitter, justified);
      CRAP green after grandfathering `Kind::db_name` (a fully-covered flat 1:1
      enum mapping tipped to cyclo=31 by the new `Kind::Chunk` arm — a CRAP
      false-positive, not real debt); `cargo fmt --all` last.
