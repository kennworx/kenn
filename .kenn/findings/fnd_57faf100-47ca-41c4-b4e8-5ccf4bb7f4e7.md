---
id: fnd_57faf100-47ca-41c4-b4e8-5ccf4bb7f4e7
tags:
- directive
- polarity:do
- supersedes:fnd_a5347cfd-b5a6-4948-99f6-15e84342a03b
parent_ids:
- fnd_a5347cfd-b5a6-4948-99f6-15e84342a03b
- fnd_484808af-f494-4525-b6e9-c4386973b957
created_at: 2026-06-14T18:37:15.537845Z
---
Markdown inline-link resolution (`resolve_inline`) ladder, in order, with directory-locality as the DEFAULT disambiguator (never a global keep-all): (1) exact path as written (by_path) → Exact; (2) path joined onto the linking file's dir and normalized via `join_relative` (strip ./, pop ..) → by_path → Exact — inline links are written relative to the linking file; (3) STALE/mirror fallback: take same-basename candidates (`by_stem`), narrow to those whose relpath ends (on a `/` boundary, case-insensitive) with the link's fuller relative suffix (`relative_suffix` = target minus leading ./ and ../, e.g. `../react-testing/SKILL.md` → `react-testing/SKILL.md`), then pick the candidate nearest the linking file by `nearest_by_locality` (longest `/`-segment common prefix — i.e. walk up the hierarchy); single → Drifted, only a true locality tie stays Ambiguous; (4) nothing → dangling stub. The suffix+locality step is what makes a real repo usable: indexing github.com/affaan-m/ecc (which mirrors the same skill filenames across .claude/.cursor/.kiro/.gemini/…, 868 files named SKILL.md) went 71692 ambiguous edges (global basename keep-all) → 5634 (relative-join only) → 38 (suffix+locality); links_to 72316 → 1417. Wikilinks `[[name]]` are intentionally name-only and still keep-all on duplication (Obsidian semantics) — locality applies to INLINE links, which carry a path.