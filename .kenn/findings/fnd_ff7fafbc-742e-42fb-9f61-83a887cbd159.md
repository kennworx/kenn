---
id: fnd_ff7fafbc-742e-42fb-9f61-83a887cbd159
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-24T17:04:32.918033Z
---
The SCIP ingest path populates pkg_id from the MONIKER — never leave it 0. It hardcoded 0 for every symbol it produced, so the `packages` table was empty for rust, go and python alike; `--package` filters matched nothing, `anchor_name_for`'s packages branch was dead code, and the atlas had to infer packages from manifest directories. The identity was in the moniker the whole time: `kenn_model::id::package::package_of` returns it per language — the crate for Rust, the distribution for Python (scip-python's head), and for Go the descriptor's LEADING NAMESPACE, because scip-go's head is the MODULE and one module covers every package in it (taking the head collapses spf13/afero's eight importable packages into one, the same mistake manifest anchoring makes).

CONSEQUENCE TO KNOW: populating pkg_id activates `anchor_name_for`'s first branch, which had never fired on the SCIP path. Go anchors therefore became full import paths (`github.com/spf13/afero/mem`) instead of the module-relative form PackageLayout synthesized (`afero/mem`). Rust, TypeScript and C# are unaffected — a crate name IS its head package. That also supersedes the filesystem-derived Go anchoring for anchoring purposes, resolving the tension noted against fnd_eaaf8c6f: package naming now comes from indexer data, not from re-deriving it in the Rust PackageLayout.