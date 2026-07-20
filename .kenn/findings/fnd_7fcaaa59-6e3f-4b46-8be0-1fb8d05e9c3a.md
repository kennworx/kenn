---
id: fnd_7fcaaa59-6e3f-4b46-8be0-1fb8d05e9c3a
tags:
- bug
- indexer
- deferred
parent_ids: []
created_at: 2026-07-04T16:54:42.459441Z
---
Producer bug (deferred): the def aggregation emits spurious zero-range def rows (file_id != 0, start_line == 0, end_line == 0) into the defs table, IN ADDITION TO the real def (81 symbols) or as the ONLY def (115 symbols). Affects both Rust (SCIP/rust-analyzer) and TypeScript symbols — e.g. rs:kenn-mcp::tools::state::ServerState had both a 19:19 def and a 0:0 def. Only-zero kinds: type_alias(49) method(34) field(18) module(14); module 0:0 may be legitimate. ~196 rows in a full self-index. The read path (kenn-mcp first_def_location_string + defs_for_symbol) was hardened to skip these (require file_id!=0 && start_line>=1), which fixed a debug-build panic (debug_assert) and a wrong #0 on the wire. Remaining work: stop emitting zero-range defs in kenn-indexer def aggregation (SCIP/JSONL parse path).