# Design — def body extent

## Decision 1: store the extent, don't parse at query time

`get_source` runs against the published snapshot; re-running rust-analyzer (or
any parser) per query is a non-starter. The body extent must be **persisted at
index time**. Two prototype alternatives were rejected:

- **Query-time brace scanner** (a ~50-line lexer-lite in `get_source`). Rejected:
  it is *wrong for Python* (indentation, not braces — it runs to EOF), fragile on
  raw strings / char-literal braces, and re-derives a value SCIP already hands
  us. It only ever made sense as a Rust-specific stopgap.
- **tree-sitter at query time.** Correct for all languages, but pulls ~5–6
  grammar crates (Swift's is unofficial) into a deliberately-lean workspace, needs
  per-language "which node is the item" mapping, and re-parses whole files — all
  to recover data the producers already compute. Revisit only if kenn wants
  syntax-aware parsing generally.

## Decision 2: a distinct span, not an overload of the name range

The def's existing `(start_line, start_col, end_line, end_col)` is the **name
span** and has its own consumers: `find_at_location` resolves a cursor position
→ symbol by matching *inside* it, and edge anchoring / location display key off
it. Stretching `end_line/end_col` to the closing brace would make
`find_at_location` resolve any position in a function body to the function
itself — a regression. SCIP keeps `range` (name) and `enclosing_range` (body)
separate for exactly this reason; the store mirrors that with separate columns.

A side table (`def_bodies(sym_id, …)`) was considered and rejected: more schema
and a join, for no benefit over two columns on `defs`.

## Decision 3: lines only, no columns

`get_source` slices **whole lines** (`slice_lines` ignores columns), so an
intra-line column on the body extent has no consumer. Store
`body_start_line` / `body_end_line` only. (The name span keeps its columns —
`find_at_location` and anchoring need them.)

## Decision 4: the extent includes doc comments / attributes

rust-analyzer's `definition_body` is "the nearest non-trivial enclosing AST
node… from `fn` to the closing brace," excluding whitespace/ordinary comments
but **keeping outer doc comments** (and, observed, attributes). So the body span
often *starts above* the name line. `get_source` uses the body span for both
bounds (start and end), returning the item with its doc comment / attributes —
useful context for an agent vetting the symbol. Verified: `transform_document`
→ `42–237` (first line `#[expect(`); `get_source` → `141–205` (first line `///`).

## Decision 5: absent extent → declaration-line fallback, never a parser

`body_start_line == 0` (old rust-analyzer, synthetic symbol, un-migrated
producer, Swift) means "no extent." `get_source` falls back to the name span —
the pre-change behavior. Honest, no heuristic, and it silently upgrades to
full-item once the producer supplies an extent. The selection rule:

```
if body_end_line >= body_start_line && body_start_line >= 1 { use body span }
else { use name span }
```

## Decision 6: per-producer sourcing

| producer | wire | body extent source |
|----------|------|--------------------|
| rust-analyzer (SCIP) | `Occurrence.enclosing_range` | read in `walk.rs` |
| scip-go / scip-python (SCIP) | `Occurrence.enclosing_range` | same code path |
| kenn-dotnet (JSONL) | new `body` range on symbol frame | `RangeUtil.FromSyntaxNode(declRef.GetSyntax())` |
| kenn-ts (JSONL) | new `body` range on symbol frame | `rangeOf(sf, decl)` |
| kenn-swift (JSONL) | new `body` range on symbol frame | SwiftSyntax declaration-node span (libIndexStore has no extent) |

Markdown / HTML / CSS already carry real multi-line def ranges (sections, rules)
and need no body extent — their name-span fallback *is* the item.

## Decision 7: rust-analyzer version is an operational constraint

The feature exists upstream; the gate is the *installed* binary. The
rustup-bundled RA tracks the Rust release and lags (stable 1.89 shipped an
Aug-2025 RA with no `enclosing_range`); the standalone / Homebrew build is
current. kenn resolves `rust-analyzer` via PATH (`command[0]`), so the fix is
operational, not a fork:

- Add a **capability probe**: if a completed Rust index produced zero
  `enclosing_range`-derived extents across definitions, log a one-time warning
  naming the too-old rust-analyzer and the upgrade (Homebrew `rust-analyzer`, or
  `rustup update`). Rust `get_source` falls back to declaration lines until then.
- Install docs recommend a recent standalone rust-analyzer.

## Decision 8: Swift uses SwiftSyntax, not libIndexStore, for the extent

kenn-swift reads the semantic index (libIndexStore), whose occurrences are
**point-based** — a name location, no extent. Rather than a fragile brace
scanner (Swift string interpolation `\(…)`, multiline strings, nested block
comments make it error-prone) or the heavier sourcekitd, kenn-swift parses each
def-bearing file once with **SwiftSyntax** — the official Swift parser, and the
direct analog of Roslyn (C#) and the TS compiler API (TS). It maps each
declaration's name-token line to the whole declaration span and emits it as
`body`. Cost: a `swift-syntax` dependency (mitigated by prebuilt macro
binaries). This keeps every one of our JSONL producers on its language's real
parser for the declaration extent.

## Non-goals

- Changing the name span, `find_at_location`, edge anchoring, or location
  rendering (`#L` / `#L-L`) — all keep using the name span.
- A wire/schema version bump — shapes change in place while prototyping.
