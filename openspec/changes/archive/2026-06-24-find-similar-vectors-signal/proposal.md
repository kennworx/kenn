## Why

Surfaced dogfooding `audit`/`dup` on a freshly-indexed (not-yet-embedded) repo:
`find_similar` returned an **empty list** for every symbol, indistinguishable from
"no similar code found." The duplication leg of `audit` and `dup` silently
produced nothing, with no hint that `kenn embed` was the missing step. Root cause:
`find_similar_symbols` returns `Ok(Vec::new())` both when the source symbol has no
committed vector (vectors not built) and when it has a vector but no near
neighbours — two very different situations collapsed to the same empty result.

Confirmed live after building vectors: `find_similar` on `PriceConverter` returns
real parallel implementations (`HistoryPriceConverter`, …), while symbols without
a committed vector still returned a bare `[]`.

## What Changes

- `find_similar_symbols` returns `Option`: `None` when the source has no committed
  vector, `Some(vec)` (possibly empty) when it does.
- The `find_similar` MCP tool maps `None` to an `EMBEDDING_UNAVAILABLE` error with
  an actionable message ("run `kenn embed`"), distinct from an empty result. A new
  `McpErrorCode::EmbeddingUnavailable` (JSON-RPC `-32002`, the service-unavailable
  family) carries it. The tool description documents the distinction.
- Regression test: a symbol with a committed vector returns `Some` neighbours; a
  symbol with no vector returns `None`.

## Capabilities

### Modified Capabilities

- `mcp-symbol-search`: `find_similar` SHALL signal a missing committed vector
  distinctly from an empty result.

## Impact

- **Bugfix / clarity** — removes a silent trap. The only behavior change is that a
  no-vector symbol now errors (actionably) instead of returning `[]`; the
  has-vector path is unchanged. No schema change.
