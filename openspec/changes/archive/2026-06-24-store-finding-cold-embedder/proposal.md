## Why

Surfaced by dogfooding `reconcile` on a large (71k-symbol) freshly-indexed repo:
the **first** `store_finding` call failed with `EmbedderStarting` ("embedder
warming up") and the finding was lost. `store_finding` pre-embeds the text for an
**advisory** near-duplicate probe (`crates/kenn-mcp/src/tools/findings.rs`), but
propagates a cold-embedder error with `?` — so on a repo whose embeddings aren't
built yet, you cannot write a finding until the embedder warms. `find_directives`
already degrades its semantic leg to `None` on `EmbedderStarting`; `store_finding`
should do the same, because the near-duplicate probe is a convenience, not a
precondition for writing.

## What Changes

- `store_finding` treats `EmbedderStarting` from the pre-embed as "skip the
  near-duplicate probe" (`text_vec = None`) rather than failing the write —
  matching `find_directives`' non-blocking degrade. The finding is written; the
  `similar` list is simply empty until the embedder is warm.

## Capabilities

### Modified Capabilities

- `findings-mcp`: `store_finding` SHALL succeed while the embedder is cold,
  skipping the near-duplicate probe.

## Impact

- **Bugfix** — a one-call error-mapping change in `tools/findings.rs`; no API or
  schema change. Removes a first-use cliff on freshly-indexed repos.
