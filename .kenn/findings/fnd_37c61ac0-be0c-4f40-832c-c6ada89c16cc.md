---
id: fnd_37c61ac0-be0c-4f40-832c-c6ada89c16cc
tags:
- directive
- guide
parent_ids: []
created_at: 2026-08-15T12:33:04.407786Z
---
The code->SQL pass drops an ENTIRE string literal when any statement in it fails to parse (refs_of_literal: 'if ex.unparsed > 0 { return Vec::new(); }'). That is deliberate — a partial parse is where a CTE or alias name gets read as a table — but it misfires badly on schema constants. sqlparser cannot parse CREATE VIRTUAL TABLE ... USING fts5(words, tokenize='unicode61') or USING vec0(embedding float[768]) in ANY of the 14 dialects (the named argument is the parse error), so kenn-store's own GRAPH_DDL const contributes zero declarations and all 15 tables it creates — symbols, defs, edges, files, packages, aggregate_*, analysis_* — are reported by 'kenn tables' as external (referenced-only) rather than declared in-repo. Verified with a sqlparser spike, not inferred. Any SQLite codebase using FTS5 or sqlite-vec hits this. A fix must not simply relax the all-or-nothing rule: the distinction that matters is a COMPLETE multi-statement DDL blob (drop only the statements that fail) versus a runtime-assembled query fragment (keep dropping the whole thing).