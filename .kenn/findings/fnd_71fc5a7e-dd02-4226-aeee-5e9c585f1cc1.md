---
id: fnd_71fc5a7e-dd02-4226-aeee-5e9c585f1cc1
tags:
- guide
parent_ids:
- fnd_484808af-f494-4525-b6e9-c4386973b957
- fnd_8e0f9755-0096-400f-8381-d7e54bff790c
created_at: 2026-06-14T09:39:17.748732Z
---
Markdown ingest is two-phase across the code join barrier (design D4). Phase 1 `ingest_markdown_phase1` runs in the ingest thread-scope: it emits md nodes + md↔md edges, and for links that fail md↔md resolution it DEFERS in-repo ones (carried out in `MarkdownPending.deferred`) while dangling external-vault ones immediately (D6 — vaults get no code resolution). The post-code-barrier `resolve_markdown_code` (run after all code units join, before aggregate/finalize, on a runtime-free thread) resolves the deferred links against the building store via `StoreCodeLookup` over `reader_from_writer`, then dangles whatever still misses. `MarkdownPending` also carries the `MarkdownIds` allocator so external-stub ids minted post-barrier never collide with phase-1 node ids. The code-lookup queries are `DbReader` inherent methods (`files_by_basename` / `symbols_by_short_name`), deliberately NOT on the `Reader` trait (they serve the indexer barrier, not the MCP hot path), and both exclude `external` + `markdown` rows so an md link only resolves to real in-workspace code.