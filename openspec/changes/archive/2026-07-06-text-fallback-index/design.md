# Design

## D1 — No tree-sitter: a size-bounded recursive splitter

The competitor's splitter is a DP over tree-sitter syntax levels. kenn has no
tree-sitter layer, and the target files (yaml/json/toml/txt) would gain little
from an AST even if one existed. So the fallback uses a **plain recursive
splitter**: split on the strongest available boundary that keeps chunks under a
target size — blank-line runs → single newlines → hard character cut — with a
small overlap. Constants mirror sensible defaults (target ~1000 chars, overlap
~150), tunable later if a corpus measurement warrants. This is intentionally the
"dumb but sufficient" tool; the value is coverage, not chunk sophistication.

## D2 — Opt-in, explicit globs, no double-indexing

Every kenn language is off by default; the fallback is too. It runs from explicit
include/exclude globs in its own config block. Two guards prevent overlap with
real producers:
1. The fallback SHALL skip any file whose extension is claimed by an enabled
   semantic/native producer (`.rs`, `.md`, `.css`, …), so nothing is indexed
   twice.
2. Discovery honors the same exclude/`.gitignore` rules as other walkers.

Explicit opt-in (rather than "index everything not otherwise handled") keeps the
blast radius controlled and avoids surprising the user with embedded
`node_modules` JSON or lockfiles.

## D3 — Node shape reuses the markdown template

Markdown already models "a file node + prose sub-nodes emitted directly as
`kenn_model` records (sibling to SCIP)". The fallback follows it exactly:
- file node: `text:<root-label>/<relpath>`, `Kind::Document`.
- chunk node: `text:<root-label>/<relpath>#<chunk-index>`, with `contains` /
  `defined_in` edges to the file, def-span = the chunk's line range, and the
  chunk text as the `SymbolDocsRecord.doc` (the embeddable/FTS unit).
No frontmatter, no link graph — those are markdown-specific.

## D4 — Registration goes through the consolidated function (see `index-producer-parity`)

Producer registration is unified into one function by the `index-producer-parity`
change (it collapses the drifted `build_driver` / `configure_runner` pair). The
text-fallback producer adds a single `with_text(...)` line there, so it runs on
both the CLI and workflow/MCP paths without a chance to drift. This change does
not re-describe that fix; it depends on it.

## D5 — Open questions

- Chunk-node identity by index (`#0`, `#1`) is stable only if the splitter is
  deterministic and the file is unchanged; a content edit reflows all subsequent
  indices. Acceptable (kenn re-indexes whole files), but if stable per-chunk
  identity across edits is ever wanted, switch to a content-hash suffix.
- Should the fallback embed the *whole* small file as one chunk when it is under
  the target size (avoiding a pointless 1-chunk split)? Yes — a file below the
  min size is a single node.
