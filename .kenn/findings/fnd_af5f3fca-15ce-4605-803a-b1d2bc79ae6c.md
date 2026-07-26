---
id: fnd_af5f3fca-15ce-4605-803a-b1d2bc79ae6c
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-24T17:04:33.31712Z
---
Traversal narrowing goes through RowNarrow and is applied STORE-SIDE, before `limit`. `list_relation` used to read only `include_external`/`include_tests` off `Filters` — `package`, `kind` and `language` were accepted by every `kenn list` subcommand and silently dropped, so a narrowed query returned the unnarrowed list and was indistinguishable from a real answer. (`find usages` was unaffected; it applies them via `passes_narrowing`, which is exactly how the two surfaces drifted.)

Two properties are load-bearing. The filter runs BEFORE the `limit` is taken, inside `list_edges`, so a narrowed page is still a full page and the pagination cursor stays correct — filtering after the fact would return short pages and a wrong `next`. And package NAMES resolve to ids ONCE per traversal (`resolve_package_ids`), not per row, because SymbolRow carries pkg_id and not the name; a name matching no package yields an EMPTY id set which must match NOTHING — treating empty as "no filter" would make a typo silently return everything.

`file`/path narrowing is deliberately still unapplied at this layer: the def path is not on SymbolRow, so it needs a join per row. Do not "fix" it with a post-hoc filter in the tool — that breaks the page-fullness property above.