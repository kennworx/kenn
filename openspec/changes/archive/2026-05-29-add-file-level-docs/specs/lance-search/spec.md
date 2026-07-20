## ADDED Requirements

### Requirement: File docs are indexed as path-identified doc rows

The search-store build SHALL join each `file_docs` row to its `files` row (`file_id → files.id`) for path and language, and emit one `Doc`-kind search row feeding the same BM25 doc inverted index as symbol docs. The row SHALL set `pub_id` empty, `path` to the file's normalized workspace-relative path (the same value stored on the `files` row), `embed_key = "filedoc:<lang>:<path>"` (the `filedoc:` prefix keeps it disjoint from symbol-doc `embed_key`s `doc:<lang>:<pub_id>`), `doc_text` to the file's joined doc text, `row_kind = doc`, and `id` to the file's **real `id`** (the file dataset's join key). Because file and symbol ids are independent id spaces, the `id` on a file row is only meaningful against the `files` dataset — hydration MUST resolve it there, not against `SYMBOLS` (see mcp-symbol-search). No separate index or text-analysis path SHALL be introduced — file doc rows are ordinary doc rows distinguished by their empty `pub_id` / path identity.

#### Scenario: A file doc becomes a BM25-searchable doc row

- **GIVEN** a `file_docs` row for `src/OrderIntake.cs` with doc text `"Handles order intake validation."`
- **WHEN** the search store is built
- **THEN** the search dataset contains a `row_kind = doc` row with `pub_id` empty, `path = "src/OrderIntake.cs"`, `embed_key = "filedoc:csharp:src/OrderIntake.cs"`, and `doc_text = "Handles order intake validation."`
- **AND** a BM25 query for `order intake` matches that row

#### Scenario: File doc rows reconcile by fingerprint like symbol docs

- **WHEN** a rebuild runs and a file's doc text is unchanged
- **THEN** the row's `embed_key` and `xxh3-64` text fingerprint are unchanged and any committed embedding is reused, identically to symbol doc rows
