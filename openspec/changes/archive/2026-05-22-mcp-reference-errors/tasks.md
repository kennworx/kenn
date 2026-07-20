## 1. Reference validation

- [x] 1.1 `find_at_location` takes `file_path` (workspace-relative or absolute) and no numeric `file_id`; `kenn-store::fetch_file_short_id` resolves a relative or absolute path — exact match, then trailing component-suffix match (design D3).
- [x] 1.2 Every navigate tool — `list_callers` / `list_callees` / `list_implementers` / `list_overrides` / `list_usages` / `list_imports` / `list_in_scope` / `list_correspondences` (via `list_relation`), `list_module_files`, `find_similar` — returns `INVALID_INPUT` for an unknown symbol id instead of an empty result (design D1).
- [x] 1.3 `store_finding` and `merge_findings` validate their id inputs and report every unresolved `fnd_…` id in one error; code-node parents pass through (design D1/D2).
- [x] 1.4 `find_predecessors` / `find_successors` reject an unknown `fnd_…` start id; a code-node start id is accepted as-is (design D2).

## 2. Verification

- [x] 2.1 `cargo clippy --workspace --all-targets` to zero warnings.
- [x] 2.2 Test (`kenn-mcp` `end_to_end`): every reference-taking tool errors on an unknown reference; `merge_findings` / `store_finding` report all bad ids, and a code-node parent is accepted, not flagged.
