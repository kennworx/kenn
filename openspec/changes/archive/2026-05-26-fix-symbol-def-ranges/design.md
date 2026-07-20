## Context

`def_range` is the four-tuple `(start_line, start_col, end_line, end_col)` stored in the `defs` dataset for every symbol. Today it is populated incorrectly on both ingest paths:

- **Rust (SCIP)**: `crates/kenn-indexer/src/transform.rs:405` pushes `DefRecord { start_line: 0, end_line: 0, start_col: 0, end_col: 0 }`. The comment claims "the SCIP path populates the actual range when the def-occurrence is seen later" — but no such back-fill exists. Every Rust symbol has `def_range = [0,0,0,0]`.
- **C# (JSONL)**: `RangeUtil.cs` faithfully copies Roslyn's 0-based `LinePosition.Line` into the wire frame, and `transform_jsonl.rs:543` stores it as-is. The reader, `slice_lines` in `crates/kenn-mcp/src/tools.rs:1822`, treats stored line numbers as 1-based (`skip(start - 1)`). Every C# symbol's `get_source` returns the line *just before* the declaration — typically the closing `/// </summary>` of the doc comment.

There is no spec that pins down stored line-basing. The wire-side spec (`dotnet-stream-indexer`) says producer emits 0-based; nothing downstream says what the store should hold or what the reader should assume.

Blast radius: `get_source`, `find_at_location` (via `enclosing.rs::smallest_enclosing`), and any future tool that materializes a body from a symbol id. The wire location format `<path>#<line>` is also silently off.

## Goals / Non-Goals

**Goals:**

- `get_source(symbol)` returns at least the symbol's declaration line (not the line before, not a doc-comment fragment) for both Rust and C#.
- `find_at_location(file, line)` returns the right enclosing symbol for both languages.
- The stored basing convention is documented in `source-data-model` so producer and reader can't drift again.
- Existing tests for `find_at_location` and `get_source` pass; new tests pin the regressions.

**Non-Goals:**

- Widening `def_range` from the **identifier-token span** to the **full declaration body**. The current C# spec mandates `ISymbol.Locations[0]` (identifier-token); SCIP definition occurrences are also identifier-token by convention. Keeping symmetry is the cheap correct fix. A later change can widen this if needed by callers.
- Changing the on-disk schema. `defs` rows keep their u32×4 shape.
- Backfilling existing indexes. Users must reindex after the fix; this is a corruption-cleanup, not a migration.
- Touching other range-carrying frames (edges, errors) that already work. The fix is scoped to symbol `def_range`.

## Decisions

### Decision 1: Store `def_range` as 1-based lines, 0-based columns

**What:** The `defs.start_line` / `end_line` columns in the store hold **1-based** line numbers (the editor convention). `start_col` / `end_col` stay **0-based** (the producer convention). Wire format `<path>#<N>` renders the stored line as-is.

**Why:** Both producers (Roslyn, SCIP) naturally emit 0-based lines. We have to do a `+1` *somewhere*; the question is where.

- **Convert in the reader** (`slice_lines` subtracts 1): keeps store consistent with producer wire, but every consumer of `defs` has to know to do the conversion. The current `slice_lines` is already a (wrong) attempt at this — the bug is just an off-by-one.
- **Convert at ingest** (`transform.rs` / `transform_jsonl.rs` add 1): the store is the boundary; readers consume what they see. Editor-friendly line numbers in the DB, in log output, in MCP wire payloads.

We pick **convert at ingest** because:
1. The wire format already presents lines to humans (`<path>#42` is read as "line 42 in the editor"). Storing 1-based makes the rendering trivial and matches how every developer reads file:line references.
2. The bug surface is "everyone who reads `defs`", not just `get_source`. Pushing the conversion to a single ingest site is the smaller, more localized fix.
3. Existing tests in `enclosing.rs` likely embed the current (broken or inconsistent) behavior; pinning 1-based at ingest forces them to be explicit either way.

Columns stay 0-based because nothing renders them today and SCIP/Roslyn agree on the 0-basing.

**Alternatives considered:**

- Store 0-based, fix the reader. Rejected: pushes the conversion to N consumers instead of 2 producers. Also keeps the misleading `slice_lines` heuristic alive (`max(1)` masks zeros).
- Store 0-based lines AND 0-based columns, accept off-by-one in rendering. Rejected: `<path>#0` rendering for the first line of a file is hostile to humans.

### Decision 2: Rust SCIP ingest pulls range from the Definition occurrence

**What:** During SCIP transform, locate the `Occurrence` with `SymbolRole::Definition` for each emitted symbol and use its `range` as the `def_range`. Drop the placeholder push.

**Why:** SCIP files contain a `definition` `Occurrence` per defined symbol (this is exactly the range that powers `Find Definition` in scip-clients). Reading it adds no new dependency — the protobuf type is already consumed for edges. The required field is `Occurrence.range`, a 3- or 4-element `[start_line, start_col, end_line, (end_col)]` array; we transform it the same way we transform edge ranges today.

**Where in the file:** `crates/kenn-indexer/src/transform.rs` already iterates documents and occurrences to emit edges. The change is to thread the definition occurrence range to the place that today pushes a placeholder, instead of relying on a separate later pass.

**Edge cases:**

- A symbol with no SCIP definition occurrence in the indexed documents (e.g., external symbol declared in a dependency). Today these get a placeholder; after the fix they keep `def_range = [0,0,0,0]` and `is_external = true`. This matches `dotnet-stream-indexer`'s synthetic-symbol carve-out: "Synthetic symbols (the root package per assembly) MAY omit `def_range`; in that case the consumer SHALL store `[0,0,0,0]`".
- A symbol with multiple SCIP definition occurrences (partial classes in Rust are rare but possible via `cfg`). We push one `DefRecord` per definition occurrence — same plurality contract as the dotnet path.

**Alternatives considered:**

- Run a second pass that builds an `Occurrence → DefRecord` map and back-fills the symbol records (what the original comment promised). Rejected: more machinery for the same outcome; the single-pass version threads the data directly.

### Decision 3: Ingest transforms do the `+1`, wire formats stay 0-based

**What:** The `+1` happens in `transform_jsonl.rs::def_for` (C# JSONL → DefRecord) and in the new SCIP-occurrence → DefRecord path in `transform.rs` (Rust). Both wire formats (dotnet JSONL `def_range` field and SCIP `Occurrence.range`) stay 0-based on the wire — matching what Roslyn and rust-analyzer naturally emit. The conversion is symmetric: each ingest site adds `+1` to `start_line` and `end_line` before pushing the `DefRecord`.

**Why:**

- The C# JSONL wire format is currently specified as 0-based and matches Roslyn's natural emission. Changing the wire to 1-based would force a `dotnet-stream-indexer` spec edit and a producer-side `+1` with no upside.
- SCIP is a third-party format we don't control; demanding 1-based input is a non-starter.
- Both ingest transforms already exist and already touch every range field — adding `+1` there is one line per site.
- This keeps **one** boundary where basing changes: the `transform → store` step. Everything downstream of `defs` is 1-based and trusts the reader-friendly value as-is.

**Where conversion lives:**

```
SCIP wire (0-based)  ─┐
                      ├─→ transform → defs table (1-based) → MCP wire (1-based, as-is)
dotnet JSONL (0-based)─┘
```

**Trade-off:** Two `+1` sites (one per pipeline) instead of one centralized site. The alternative — one centralized normalizer — would need a `DefRecord::from_zero_based` constructor or a shared helper, which is more machinery for two callers. Inline `+1` at each push site is simpler and is exactly where a future test will catch a regression (the test reads back `def_range` for a known symbol and asserts the line).

## Risks / Trade-offs

- **Existing indexes are now formally corrupt.** → Mitigation: bump the snapshot generation; on next index status check, surface a "reindex required" hint. Users running `kenn` against a stale index get wrong answers from `get_source` today, so the worst case after the fix is "still wrong until reindex" — same blast radius.
- **The `def_range Is Populated` requirement for Rust SCIP raises the bar.** If `rust-analyzer scip` ever emits a symbol without a definition occurrence (does happen for some macro-generated items), we'll start seeing failures we previously swallowed. → Mitigation: keep the synthetic-symbol carve-out (`is_external = true`, `def_range = [0,0,0,0]` allowed). Add a test that pins this allowed shape.
- **C# producer change is breaking for any external JSONL replayer.** → No such tool exists outside the repo. The dotnet-stream-indexer spec is updated to match.
- **Behavior change for `find_at_location`** — today returns nothing useful for Rust; after the fix it returns real symbols. This is a behavior repair, not a regression, but anyone writing a test today that depends on "Rust returns nothing" will need to update.

## Migration Plan

1. Land the ingest fix (both pipelines) and the reader fix simultaneously. They're co-dependent — landing only one leaves the other off-by-one in a new direction.
2. Force a full reindex on the next `kenn` invocation by bumping the snapshot generation marker (existing mechanism).
3. Update CRAP baseline if the ingest changes shift cyclomatic complexity on `transform.rs::transform`.

No rollback strategy needed beyond `git revert` — there's no data migration, just a reindex.

## Open Questions

- Should the wire location format render single-line `def_range` as `#42` (current behavior) or `#42-42`? Source-data-model says `#42`. Keeping current.
- Does `rust-analyzer scip` emit `Definition` occurrences for `impl` block methods reliably? Need a test fixture covering trait impls.
