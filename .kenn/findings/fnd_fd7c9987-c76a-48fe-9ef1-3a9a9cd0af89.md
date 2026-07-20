---
id: fnd_fd7c9987-c76a-48fe-9ef1-3a9a9cd0af89
tags:
- gotcha
- dogfood-2026-06-24
parent_ids:
- rs:kenn-store::db::sqlite::reader::search::`SqliteConnRef<'_>`::find_similar_symbols
- rs:kenn-mcp::tools::query::find_similar
created_at: 2026-06-24T16:28:45.13164Z
---
GOTCHA (fixed): find_similar must distinguish "source symbol has no committed vector" from "no similar symbols found" — previously both were Ok(Vec::new()), a silent trap. find_similar_symbols now returns Option: None when the source has no committed embedding, Some(vec) (possibly empty) when it does. The MCP find_similar tool maps None to a new EMBEDDING_UNAVAILABLE error (-32002) naming `kenn embed`. Without this, the audit/dup duplication leg silently produces NOTHING on a freshly-indexed but un-embedded repo, with no hint that `kenn embed` is the missing step. Found dogfooding the audit and dup skills on a large external C# repo (find_similar returned [] for every symbol until vectors were built).