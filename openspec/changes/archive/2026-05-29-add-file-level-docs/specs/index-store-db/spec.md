## ADDED Requirements

### Requirement: file_docs dataset stores per-file comment text

The consumer SHALL maintain a sparse `file_docs` dataset with one row per file that has a surviving file-level doc: `{ file_id, doc: <Utf8> }`, where `file_id` references `files.id`. A file with no surviving doc SHALL have no `file_docs` row (sparse, mirroring `symbol_docs`). The dataset SHALL be distinct from both `files` (docs are not a dense column on the file row) and `symbol_docs` (a file is not a symbol), leaving room to index file source bodies under separate storage in a future change.

#### Scenario: A file with a useful header gets one file_docs row

- **WHEN** a `FileFrame.doc` array survives license filtering with non-empty text
- **THEN** exactly one `file_docs` row is written keyed by the file's `id` (column `file_id`)
- **AND** its `doc` is the surviving entries joined with a blank-line separator

#### Scenario: A file whose only comment is a license header gets no row

- **WHEN** every entry in `FileFrame.doc` matches the license-boilerplate heuristic
- **THEN** no `file_docs` row is written for that file

### Requirement: License-boilerplate filtering on file-doc ingest

When converting a `FileFrame.doc` array to a `file_docs` row, the consumer SHALL drop entries that match a conservative license/boilerplate heuristic and keep the rest. The heuristic SHALL match (case-insensitively) at least: `SPDX-License-Identifier`, `Copyright (c)` / `Copyright ©`, `All rights reserved`, `Licensed under`, `Permission is hereby granted`, and the canonical opening phrasing of the GPL, Apache-2.0, and MIT licenses. Filtering SHALL be per-entry — a matching entry is dropped without discarding sibling entries.

#### Scenario: License entry dropped, purpose entry kept

- **GIVEN** a `FileFrame.doc` of `["// Copyright (c) 2026 Acme. All rights reserved.", "// Handles order intake validation."]`
- **WHEN** the consumer builds the file_docs row
- **THEN** the stored `doc` is `"// Handles order intake validation."` only
