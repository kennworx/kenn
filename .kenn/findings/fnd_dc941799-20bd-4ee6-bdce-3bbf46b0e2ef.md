---
id: fnd_dc941799-20bd-4ee6-bdce-3bbf46b0e2ef
tags:
- directive
- polarity:dont
- supersedes:fnd_ac8f404a-5fb4-447a-af27-cefa78aaa76c
parent_ids:
- fnd_ac8f404a-5fb4-447a-af27-cefa78aaa76c
created_at: 2026-07-05T17:20:41.232365Z
---
Index-run setup is shared between the CLI ('kenn index', crates/kenn-cli/src/cmd_index.rs) and the workflow/MCP index_workspace path (crates/kenn-indexer/src/workflow.rs) via TWO single-source functions in kenn-indexer: build_workspace (workspace excludes + test globs + per-language excludes) and configure_runner (producer/driver registration). Both were previously duplicated per-entry-path and BOTH silently drifted (configure_runner dropped markdown + dotnet test_globs from the MCP path; build_workspace dropped Swift language-excludes). Do NOT reintroduce a per-entry-path copy of either — add any language, producer, or exclude ONCE in the shared function. Guarded by crates/kenn-indexer/tests/markdown_producer_parity.rs and build_workspace_parity.rs.