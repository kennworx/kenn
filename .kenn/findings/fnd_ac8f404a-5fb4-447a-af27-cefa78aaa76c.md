---
id: fnd_ac8f404a-5fb4-447a-af27-cefa78aaa76c
tags:
- directive
- polarity:dont
- supersedes:fnd_7bd76975-fd9a-435f-81e8-cb39ae370518
parent_ids:
- fnd_7bd76975-fd9a-435f-81e8-cb39ae370518
created_at: 2026-07-05T16:41:58.09052Z
---
Producer registration for indexing is a SINGLE source of truth: configure_runner (crates/kenn-indexer/src/workflow.rs), re-exported from kenn_indexer and called by BOTH the CLI ('kenn index', cmd_index.rs) and the workflow/MCP index_workspace path. Add any new language/producer here, once. Do NOT reintroduce a second driver-building function — the previous build_driver/configure_runner split drifted and silently dropped markdown (and dotnet test_globs) from the MCP path. Guarded by crates/kenn-indexer/tests/markdown_producer_parity.rs.