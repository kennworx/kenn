## Why

`get_source` returns wrong content for every indexed symbol, and `find_at_location` silently misroutes Rust queries. Both stem from broken `def_range` data: the Rust indexer pushes a `[0,0,0,0]` placeholder that nothing ever back-fills, and the C# pipeline plus MCP reader disagree on whether stored line numbers are 0-based or 1-based.

## What Changes

- Rust SCIP indexer: populate `DefRecord.{start_line, start_col, end_line, end_col}` from the SCIP definition-occurrence range instead of pushing zeros. Remove the stale "populates the actual range when the def-occurrence is seen later" comment.
- C# / Rust / reader: pin a single line-basing convention for stored `def_range`. Producers normalize on the way in; the reader is the dumb consumer.
- MCP `get_source` and the location-rendering helper: consume the agreed basing without `max(1)` / `skip(start - 1)` heuristics that silently mask off-by-ones.
- Specs: tighten `source-data-model` to state the stored line-basing explicitly, and tighten `scip-indexer` to require non-zero `def_range` for every non-synthetic symbol (matching the existing `dotnet-stream-indexer` requirement).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `scip-indexer`: add a `def_range Is Populated` requirement parallel to the one already in `dotnet-stream-indexer`. Today the spec is silent on this and the implementation ships zeros.
- `source-data-model`: state the line-basing of stored `def_range` (currently undefined; producers disagree with the reader). Also clarify that wire `#<line>` rendering uses the stored value as-is once basing is pinned.

### Affected (no spec change, but implementation touched)

- `mcp-server`: `get_source` and the `<path>#<start>-<end>` location renderer. Wire shape unchanged; the values flowing through become correct.
- `dotnet-stream-indexer`: no spec change. The wire-side `def_range Is Populated` requirement stays 0-based; ingest conversion to 1-based happens in `transform_jsonl.rs` and is governed by `source-data-model`.

## Impact

- **Code**:
  - `crates/kenn-indexer/src/transform.rs` — populate Rust DefRecord from SCIP occurrence range
  - `crates/kenn-indexer/src/transform_jsonl.rs` — possibly add basing conversion (depends on design decision)
  - `crates/kenn-mcp/src/tools.rs::slice_lines` — remove the `max(1)` / `skip(start - 1)` off-by-one assumption
  - `crates/kenn-mcp/src/server.rs` — `find_at_location` tool description currently says "0-based line number"; flip to 1-based so the agent-visible contract matches the new stored basing
  - `indexers/kenn-dotnet/src/Indexing/RangeUtil.cs` — possibly add basing conversion
- **Tools affected (no API change, behavior repair)**: `get_source`, `find_at_location`, and any symbol-location rendering that passes through `defs`.
- **Data**: existing indexes are wrong; users will need to reindex after the fix lands. No on-disk schema change.
- **Dependencies**: none.
- **Tests**: add coverage that `get_source(known_symbol)` returns the symbol's actual signature line, in both Rust and C# fixtures. Add coverage that `find_at_location` returns the expected enclosing symbol for both languages.
