# Tasks — def body extent

> **Complete.** Implemented and verified end-to-end for Rust, C#, TypeScript, and
> Swift (Go / Python inherit the SCIP path). Rust `def_bodies_seen` 6,147/6,331 on
> a self-index; C# / Swift proven via a real index → `get source`. Gates green
> (clippy / CRAP / fmt / dotnet format / swift tests).

## 1. Store & model — `[x]`

- [x] `DefRecord` (`kenn-model`): add `body_start_line` / `body_end_line` (u32).
- [x] `DefRow` + `DefLineRow` (`kenn-store::api::types`): add the two fields.
- [x] `defs` schema: two `INTEGER NOT NULL DEFAULT 0` columns.
- [x] writer insert (`writer/core.rs`): carry the two columns.
- [x] reader (`reader/fetch.rs`): `SELECT` + `DefRow`/`DefLineRow` projection.
      → verify: `SELECT sum(body_end_line>0) FROM defs` > 0 after a Rust reindex.

## 2. SCIP capture (Rust / Go / Python) — `[x]` spike-done

- [x] `collect_definition_occurrences` (`walk.rs`): capture
      `Occurrence.enclosing_range` alongside the name range.
- [x] `push_def_records`: convert the enclosing range to a 1-based body span;
      `0` when absent or synthetic.
      → verify: `get source rs:…::transform_document` returns the whole item.

## 3. `get_source` + brace-scanner removal — `[x]` spike-done

- [x] `get_source`: slice the stored body extent when present, else the name span.
- [x] Remove the prototype `item_end_line` brace scanner + its tests.

## 4. C# producer (kenn-dotnet) — `[x]`

- [x] `SymbolFrame` (`Wire/Frames.cs`): add an optional `body` range field.
- [x] `IndexerCore.cs`: emit `RangeUtil.FromSyntaxNode(declRef.GetSyntax())`
      (full declaration node span) as `body`.
- [x] `jsonl-indexer-driver` ingest (`parse_jsonl` + `transform_jsonl/stream.rs`):
      read `body` → def body span; absent → `0`.
- [x] xunit coverage; `dotnet format` last.

## 5. TypeScript producer (kenn-ts) — `[x]`

- [x] Symbol frame: add a `body` range; emit `rangeOf(sf, decl)` (whole
      declaration) in `symbols.ts`.
- [x] Ingest via the same `jsonl-indexer-driver` path as C#.

## 6. Swift producer (kenn-swift) — `[x]`

libIndexStore is point-based (a name location, no extent), so the extent is
recovered syntactically with **SwiftSyntax** (the official parser — the Swift
analog of Roslyn / the TS compiler).

- [x] Add `swift-syntax` to `Package.swift` (range spans a Swift major →
      resolves 603.x for Swift 6.3; prebuilt macro binaries keep the build fast).
- [x] `BodyExtents.swift`: parse each def-bearing file once, map name-token line
      → whole-declaration span (attributes → closing brace, excl. leading doc
      comment).
- [x] `Indexer.swift`: emit `body` from the extent; omit when the file won't
      parse or no declaration name lands on the def line.
- [x] e2e test: `struct Order` emits `body = [4,0,9,0]` (lines 5–10).
      → verify: `get source sw:SampleApp.Order` returns the whole struct.

## 7. rust-analyzer capability check — `[x]`

- [x] After a Rust index, if zero definition extents were captured, log a
      one-time warning: the installed rust-analyzer is pre-Dec-2025 and emits no
      `enclosing_range`; recommend Homebrew `rust-analyzer` or `rustup update`.
- [x] Install docs: recommend a recent standalone rust-analyzer (the
      rustup-bundled build lags).

## 8. Tests — `[x]`

- [x] `walk.rs`: enclosing_range → body span; absent → 0; synthetic → 0.
- [x] `get_source`: full item when extent present; declaration-line fallback when
      absent; doc-comment/attribute inclusion.
- [x] store round-trip: writer→reader carries body span.
- [x] Fix the ~20 `DefRecord` construction sites in tests/benches (spike patched
      only the non-test producer sites needed to compile `kenn-cli`).

## 9. Gates — `[x]`

- [x] `cargo clippy --workspace --all-targets` clean.
- [x] `just crap-ci` green.
- [x] `cargo fmt --all` last.

## 10. Docs / skill — `[x]`

- [x] Note `get_source` returns the full item (with doc comment / attributes).
- [x] Install section: recent rust-analyzer requirement for Rust extents.
