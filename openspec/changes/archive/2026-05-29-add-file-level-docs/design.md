# Design — file-level docs for C#

## Empirical grounding (Roslyn probes)

All verified against Roslyn 4.7.0 with a standalone probe:

- A `//` or `/* */` block at the **top of the file** attaches as `SingleLineCommentTrivia` / `MultiLineCommentTrivia` to `root.GetFirstToken().LeadingTrivia` (the first token is whatever comes first — usually a `using` keyword). Multi-line `/* */` is preserved as one trivia token with newlines intact.
- A comment **after the usings, before the namespace** attaches to `namespaceDecl.GetLeadingTrivia()` — *not* the first token. So capturing both means reading two slots: the first token's leading trivia (file header) and each namespace decl's leading trivia.
- `ISymbol.GetDocumentationCommentXml()` returns the `///` XML doc for types/members, but **empty for namespaces** even when a `///` is syntactically present. So namespace-leading comments are only reachable via trivia.
- C# emits a namespace `SymbolFrame` already (`WalkNamespace`, IndexerCore.cs:293), deduped across files — so it is *not* a per-file carrier. File headers need a per-file home.

## Decisions

### D1. Producer emits per-comment-block entries; no filtering
`FileFrame.doc: string[]` carries one entry **per contiguous comment block**, in source order, drawn from two slots: (1) the leading trivia of the compilation unit's first token (the file header), and (2) each namespace declaration's leading trivia. Consecutive `SingleLineCommentTrivia` are coalesced into one block (joined by newlines); a **blank line** between them breaks the block. Each `MultiLineCommentTrivia` is its own block with newlines preserved.

Block (not per-token) granularity is **required** so the consumer's license filter can drop a *multiline* `//` license header as a unit. A real MIT/Apache header spans many `//` lines and only the first carries the copyright/license marker — the warranty/permission continuation lines do not — so a per-token filter would keep those leaked lines. Coalescing makes the whole header one entry that a single marker match drops wholesale. The cost: a purpose line glued directly under the copyright with **no** blank line is dropped with it (rare — a blank line, the common style, separates the two and splits the block).

**Dedup:** when the file's first token *is* the `namespace` keyword (no usings/types before it), slot (1) and slot (2) point at the same trivia span. The extractor SHALL emit each comment token once — skip the namespace slot for trivia already taken by the first-token slot (compare by trivia `FullSpan`).

The producer applies no license filtering — policy lives on the consumer so the same filter can later serve Rust-source file docs.

Emission timing: `FileFrame` is currently written by `FileTracker.RegisterIfNew` on first path sighting (FileTracker.cs:48), which happens during the symbol walk — *before* the per-tree loop that has the syntax `root`. Resolution: thread the owning `SyntaxTree` into `RegisterIfNew` and extract the doc array once, on first sight, via a static helper (`FileDoc.Extract(SyntaxTree)`). All three call sites already have a `SyntaxTree` in hand (IndexerCore.cs:372 `refExtra.SyntaxTree`, :411 `loc.SourceTree`, :599 `tree`).

### D2. License/boilerplate filter on the Rust consumer
In `transform_jsonl::on_file` (transform_jsonl.rs:334), each `doc` array entry (a comment *block*, D1) is tested against a conservative boilerplate heuristic; matching entries are dropped (per-block, never the whole array). A multiline `//` license arrives as one block, so a single marker match drops the entire header. Markers: `SPDX-License-Identifier`, `Copyright (c)` / `Copyright ©`, `All rights reserved`, `Licensed under`, `Permission is hereby granted`, and the canonical GPL/Apache/MIT opening phrases. Survivors are joined with `\n\n`. Empty result ⇒ no `file_docs` row.

### D3. Separate sparse `file_docs` dataset
Mirrors `symbol_docs`: one row per file that has a surviving doc, `{ file_id, doc: Utf8 }` (`file_id` FK → `files.id`). Kept separate from `files` (docs are never a dense column on their entity in this store) and from `symbol_docs` (a file is not a symbol; reusing it would require synthetic file-symbols). Separate dataset leaves room to index file *source bodies* later with independent storage and query. Added to `GraphDataset` (schema.rs:45) + `files_*`-style schema/batch builders + reader scan.

### D4. Uniform BM25 via path-identified Doc rows
`build_knowledge_store` (writer.rs:155) scans `symbols` + `symbol_docs` to build search rows. Extend it to also scan `file_docs`, join each `file_docs.file_id → files.id` for the file's path + language, and emit one `RowKind::Doc` row per file doc through `build_batch_rows` (schema.rs:234): `embed_key = "filedoc:<lang>:<path>"`, `pub_id = ""`, `path = <normalized workspace-relative path>` (fills the currently-null path-fallback column, schema.rs:293-296), `doc_text = <joined surviving doc>`. The `path` MUST be the same normalized path stored on the `files` row so the `embed_key` stays constant across runs. The `filedoc:` prefix keeps the file-doc identity disjoint from symbol-doc `embed_key`s (`doc:<lang>:<pub_id>`), so the reconciliation key can never collide. No new BM25 index — the existing doc inverted index covers these rows.

The doc text is the license-filtered survivors joined with `\n\n` (the filter is applied on ingest, D2; the search row indexes the already-clean text).

### D5. File hits in search results
A path-identified doc-row hit hydrates to a **file result**, not a symbol. The search result type (`FoundSymbolRow` / `RankedSymbolRow` / `BlendedSymbolRow` in kenn-store; the MCP response in tools.rs) gains a file variant carrying `path`, `language`, and the doc snippet. `search_symbols` / `semantic_search` responses interleave file and symbol hits by score; the row carries a discriminant so the agent can tell them apart. `find_symbol` (literal-name lookup) is **unchanged** — it never returns file hits.

### D6. File rows carry their real id; hydration branches on row kind
**Correctness-critical.** A file's `id` and a symbol's `id` come from **separate per-language 1-based counters** (transform.rs:95-99), so file `#7` and symbol `#7` coexist. BM25 hit hydration currently maps every join id `→ SYMBOLS` unconditionally (`hydrate_records`, db/reader.rs:96). The hazard is *blind* hydration, not the id itself.

Resolution: a file-doc search row carries the file's **real `id`** (the same key `defs.file_id` references, resolvable via `fetch_file_path` / a `fetch_file_row` accessor) plus `pub_id = ""` and `path` set. Hydration **partitions hits by kind before resolving**: file rows (empty `pub_id`) hydrate from the `files` dataset by `id`; symbol rows (non-empty `pub_id`) go through the existing `hydrate_records` unchanged. This is symmetric with symbols — the search `id` is the within-snapshot join key for both; only the target dataset differs. No sentinel.

`path` is NOT the hydration key — it is the file row's **stable cross-run identity** for the `embed_key` reconciliation column (file ids are rewritten every run, schema.rs:82), mirroring how a symbol row's `embed_key` is built from its `pub_id`.

### D7. No schema-version bump; reindex in place
Adding `file_docs` is a schema change, but there are no users, so we do **not** bump `STORE_SCHEMA_VERSION` or carry a compatibility path — the shape changes in place and a stale snapshot is regenerated by a reindex. (Same stance as `uniform-store-id-naming`, which this change is sequenced after.)

### D8. Cross-language asymmetry (accepted, documented)
Rust file-level docs (`//!`) already attach to the per-file **module symbol** and flow through `symbol_docs` → surface as **symbol** hits. C# has no per-file symbol, so its file docs go through `file_docs` → surface as **file** hits. "File-level docs" therefore surface differently per language. We **accept** this asymmetry for now (it mirrors the languages' actual structure — Rust files are modules, C# files are not) rather than expand scope to migrate Rust `//!` into `file_docs`. Revisit if the inconsistency proves confusing in search results.

## Reconciliation note
`build_knowledge_store` pairs a symbol's name+doc rows so their joined fingerprint matches the embedding sidecar (schema.rs:176-179). A file doc row is a **standalone doc row with no name row**; its fingerprint is over its own `doc_text` only. Verify the reconcile/reuse path handles a lone doc row (expected to be fine — fingerprint is per-row — but covered by a validation task).
