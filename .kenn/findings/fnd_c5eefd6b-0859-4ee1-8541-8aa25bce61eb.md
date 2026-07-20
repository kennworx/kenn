---
id: fnd_c5eefd6b-0859-4ee1-8541-8aa25bce61eb
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-07-06T13:33:22.829262Z
---
The text-fallback producer (kenn-indexer/src/text/) is STRUCTURE-BLIND by design: a plain recursive char/line splitter (blank-line → newline → hard-cut), NOT format-aware. It does not parse yaml/toml/json structurally, attaches no key-path context, and degrades to blind byte-cuts on single-line/minified content. This is the deliberate v1 (coverage over sophistication; kenn has no tree-sitter layer). Do NOT add tree-sitter or per-format structural chunking unless retrieval quality on config files is MEASURED as weak — the flat splitter is sufficient for the 80% win of making these files searchable at all. A structure-aware follow-up would prepend each chunk with its key-path and cut only on structural boundaries; deferred until measured.