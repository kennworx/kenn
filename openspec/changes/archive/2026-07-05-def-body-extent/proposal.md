## Why

`get_source` returns only the **declaration line** for a function, method, or
type — not the item body. For `rs:…::transform_document` it returns
`walk.rs#46` (the `pub fn transform_document(` line) instead of lines 42–237
(the whole item, attribute through closing brace). That blunts the core
"re-read the source to vet a symbol" workflow the `audit`, `dup`, and `blast`
skills rely on.

The cause is a data-model gap, not a query bug. The stored `DefRecord` range is
the SCIP **name-token span** (the identifier), which for a definition is a
single line. SCIP also carries `Occurrence.enclosing_range` — the *body* extent
(`fn` → closing brace, incl. outer doc comment and attributes) — and the
transform already reads it in `DocumentDefIndex` for edge FROM-attribution…
then **throws it away**. The stored def keeps only the name span, so
`get_source` has nothing but the declaration line to slice.

The extent is available from every producer we care about:

- **Rust** — rust-analyzer emits `enclosing_range` on definitions since Dec-2025
  (PR #21141, trivia-refined by #22595, 2026-07-04). We verified 0/19,905 on an
  Aug-2025 build and a correct `[start … close-brace]` extent on a 2026-06 build.
- **Go / Python** — scip-go / scip-python already stamp `enclosing_range` on
  definitions; the transform reads it for edges and discards it.
- **C# / TS** — our own JSONL indexers; Roslyn (`declRef.GetSyntax()`) and the
  TS AST (`rangeOf(sf, decl)`) hand us the full declaration span for one line of
  code. Today they emit the name span (`ISymbol.Locations` / `nameNode`).
- **Swift** — libIndexStore gives a location, not an extent, so kenn-swift
  parses each file with **SwiftSyntax** (the official parser) for the span.

So the fix is to **stop discarding a value the producers already compute**, not
to parse source at query time. (A prototype query-time brace scanner was
rejected: it is wrong for Python — indentation, not braces — and re-derives what
SCIP hands us. See `design.md`.)

## What Changes

- **Store**: `defs` gains two lines-only columns `body_start_line` /
  `body_end_line` (1-based; `0` = absent). `DefRecord`, `DefRow`, `DefLineRow`,
  the writer insert, and the reader select carry them.
- **SCIP transform** (`walk.rs`): capture `Occurrence.enclosing_range` for
  definition occurrences and convert to the 1-based body span. Empty extent
  (old rust-analyzer, synthetic symbol) → `0`.
- **`get_source`**: slice the stored body extent when present; fall back to the
  name span (the declaration line) when absent. No parser.
- **JSONL producers**: `dotnet-stream-indexer`, `typescript-stream-indexer`, and
  `swift-stream-indexer` emit a body range on the symbol frame
  (`RangeUtil.FromSyntaxNode(declNode)` / `rangeOf(sf, decl)` / SwiftSyntax
  declaration span); `jsonl-indexer-driver` ingests it.
- **Capability check**: warn when the resolved rust-analyzer emits no
  `enclosing_range` across definitions (i.e. a pre-Dec-2025 build), pointing the
  operator at an upgrade; Rust source falls back to declaration lines until then.

## Capabilities

### Modified Capabilities

- `source-data-model`: the def data model gains an enclosing-item body extent
  distinct from the name span, and `get_source` returns the whole item when an
  extent is stored (else the declaration line).
- `scip-indexer`: the SCIP transform maps `Occurrence.enclosing_range` onto the
  def body extent (Rust / Go / Python), and surfaces a too-old rust-analyzer.
- `dotnet-stream-indexer`, `typescript-stream-indexer`, `swift-stream-indexer`:
  symbol frames carry a `body` range (the full declaration span) alongside the
  name range.

## Impact

- **Schema:** two additive columns on `defs` (recreated fresh on every index —
  no migration). No wire-version bump (prototyping).
- **Indexing:** a reindex populates the columns. For Rust extents the resolved
  rust-analyzer must be ≥ Dec-2025 (the rustup-bundled RA lags; the standalone /
  Homebrew build is current); older builds fall back to declaration lines with a
  warning.
- **Scope:** ~1 crate each for store / indexer / mcp, plus the C# / TS / Swift
  producer emit + JSONL ingest. Swift adds a `swift-syntax` dependency.
- **Status:** implemented and verified end-to-end for **Rust, C#, TypeScript,
  and Swift** (Go / Python inherit the SCIP path). Rust: `def_bodies_seen`
  6,147/6,331 on a self-index, `get source` returns whole items; C#: a method
  returns its `[attr … closing-brace]` span; Swift: `struct Order` returns lines
  5–10. Gates (clippy / CRAP / fmt / dotnet format / swift tests) green.
