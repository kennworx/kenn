---
id: fnd_b4baf13d-adcc-4df0-a5d8-7a41f88874a1
tags:
- directive
- guide
- supersedes:fnd_37c61ac0-be0c-4f40-832c-c6ada89c16cc
parent_ids:
- fnd_37c61ac0-be0c-4f40-832c-c6ada89c16cc
created_at: 2026-08-16T07:56:27.089973Z
---
RESOLVED by the ddl-survives-a-partial-parse change: a partial parse no longer discards the whole SQL literal. It keeps the references made by statements whose verb names a schema object by grammatical position (CREATE TABLE, CREATE INDEX, ALTER TABLE, ...) and still drops queries and DML, where a torn-away WITH turns a CTE into a table. The predicate is Verb::names_positional, an exhaustive match on an enum, so a new statement kind will not compile until it is classified. Measured after the fix: kenn's own schema went from invisible to 29 of 48 tables declared in-repo, and across two corpora the relaxation admitted 27 references, ALL of them real tables (26 from GRAPH_DDL, 1 ALTER TABLE on a multi-language corpus). Keep the original measurement in mind when touching this: sqlparser rejects CREATE VIRTUAL TABLE ... USING fts5(words, tokenize='unicode61') in all 14 dialects, so one such statement used to cost the 26 readable ones beside it.