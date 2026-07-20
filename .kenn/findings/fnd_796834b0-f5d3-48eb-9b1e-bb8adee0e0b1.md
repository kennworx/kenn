---
id: fnd_796834b0-f5d3-48eb-9b1e-bb8adee0e0b1
tags:
- directive
- polarity:do
parent_ids:
- fnd_7674f12e-2cf9-45f8-bfc1-19e0702f1f60
created_at: 2026-06-09T13:43:05.748608Z
---
Cold-start (run_startup_decision in kenn-mcp indexing/orchestrate.rs) must NOT serve an empty snapshot as `Ready` when kenn.toml enables at least one language — re-index instead (stay `Indexing` until populated), so agents never see a misleading empty index that a prior transient indexer failure published under the workspace's staleness key. A workspace that genuinely expects no symbols (no kenn.toml, or all languages disabled) still settles `Ready` with the empty-snapshot config-hint and must NOT trigger a reindex loop. Gate the reindex on `config_expects_symbols` (any [language.*].enabled); it runs at most once per cold start. See `skip_or_reindex_on_empty`.