---
id: fnd_dec67fbe-608b-40f9-a5e6-3f7ecb0772ec
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-26T18:06:04.262732Z
---
One join rule for link-relative paths, in `kenn-indexer/src/relpath.rs`: `join_relative(linking_relpath, target)` pops a segment per `..`, returns None above the workspace root, handles root-relative `/…`, and rejects `\\`-separated or drive-absolute targets (they would survive as one opaque segment and escape via Path::join on Windows). Every surface that GRADES a link uses it — md<->md `resolve_inline`, md->code `resolve_file_ref`, and HTML anchors/fragments/assets. It exists because three private copies drifted: two were correct and the third (`code_resolve::normalize`) 'resolved' `..` by deleting the token, which both missed correct links (graded drifted) and matched WRONG same-basename files as exact. If you need this rule, call it — do not write a fourth copy. The exact rung takes EITHER spelling (as-written, already workspace-relative, or joined), matching resolve_inline; but where the probe hits the FILESYSTEM rather than a basename-filtered graph set, joined must be tried FIRST, or a nested `[t](docs)` binds to the repo-root `docs/`.