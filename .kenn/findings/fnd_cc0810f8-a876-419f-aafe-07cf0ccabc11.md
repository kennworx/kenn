---
id: fnd_cc0810f8-a876-419f-aafe-07cf0ccabc11
tags:
- directive
- polarity:do
- mcp
- tool-design
parent_ids: []
created_at: 2026-06-19T15:16:41.418488Z
---
MCP tool design for kenn: shape tools around agent *intents*, not graph primitives. Each MCP call is a full LLM roundtrip (latency + tokens), so the hottest paths must not require chaining. Rules:

1. FUSE common lookup-then-traverse intents into ONE tool that resolves + traverses server-side. The agent should not be the glue. E.g. `find_usages(query)` does `find_symbol`→`list_usages` in one call (see change `find-usages`).

2. SURFACE match-ambiguity IN THE RESPONSE, not via a second roundtrip: when a query resolves to N nodes, return results grouped by resolved target so the caller gets everything in one call. A second call to disambiguate is the anti-pattern.

3. Prefer a required `query` string + OPTIONAL narrowing filters (reuse the existing `kind`/`package`/`language`/`path` Filters vocab) over either:
   - splitting into per-type tools (`find_symbol_usages` / `find_file_usages`) — that only disambiguates the cheap "which table" axis (inferable from query shape anyway), not the real "which of N matches" axis, and multiplies tool surface for every fused intent; or
   - a `string | object` union argument — LLMs fumble oneOf/anyOf tool schemas. Structure should be optional narrowing ON TOP OF the string, never a replacement.

4. Keep the graph primitives (`find_symbol`, `list_usages`, `list_callers`) for stepwise/power use; the fused tool is the one-shot intent for hot paths.