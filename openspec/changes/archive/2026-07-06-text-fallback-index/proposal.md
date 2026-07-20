## Why

kenn only makes a file **semantically searchable** if a producer emits prose for
it: SCIP `documentation` for the 6 code languages, markdown section bodies, or
the preceding comment on CSS/HTML selectors. Everything else is invisible to
`semantic_search` — **yaml, json, toml, rst, plain `.txt`, and any unrecognized
source** are never read (`crates/kenn-indexer/src/*/discover.rs` are all
extension-scoped; there is no generic content walker). A competitor covers 30+
formats by chunking *any* file; kenn covers what its per-language producers reach.

This proposes a **generic text-fallback producer**: for user-configured globs,
walk arbitrary text files, split them into size-bounded chunks, and emit each
chunk as a node whose text feeds FTS + embeddings — the same node shape markdown
already uses. It is **not** tree-sitter/AST based (kenn has no tree-sitter layer,
and these formats have no grammar in kenn); a plain recursive character/line
splitter is the honest, sufficient tool for config/prose files.

**Depends on `index-producer-parity`.** The new producer must register on *both*
index entry paths (CLI and workflow/MCP). Today those paths are configured by two
drifted functions (`build_driver` / `configure_runner`); the `index-producer-parity`
change consolidates them into a single source of truth first. This change then
adds the fallback producer in that one place, so it cannot drift between paths.

## What Changes

- Add a `text-fallback` producer: config-driven include/exclude globs (off by
  default, like every kenn language), a size-bounded recursive splitter
  (target size + overlap, split on blank-line → line → hard-cut), emitting a file
  node plus one node per chunk with the chunk text as embeddable prose. Node
  identity `text:<relpath>#<chunk-index>` (mirroring markdown's slug scheme).
- The fallback SHALL NOT index a file already claimed by another producer
  (extension guard), so a `.rs` is never double-indexed via SCIP *and* fallback.
- Register the fallback producer via the single consolidated registration
  function introduced by `index-producer-parity`, so it runs on both the CLI and
  workflow/MCP entry paths.

## Capabilities

### Added Capabilities

- `text-fallback-index`: a generic recursive-split producer that makes
  user-selected non-semantic text files searchable via FTS + embeddings.

## Impact

- **Behavior:** with the fallback enabled, configured text files become nodes and
  appear in `semantic_search` / `search_symbols`. With it disabled (default),
  nothing changes.
- **Cost:** more embedded chunks when enabled (bounded by the user's globs). The
  embed/finalize path is already content-agnostic (`finalize.rs` embeds any
  non-empty `doc`), so no storage/embed changes are needed.
- **Scope guard:** this is deliberately a *fallback*, not a re-architecture — it
  does not add tree-sitter, and it does not touch the SCIP producers.
