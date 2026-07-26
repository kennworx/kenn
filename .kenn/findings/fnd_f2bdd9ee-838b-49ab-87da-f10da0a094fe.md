---
id: fnd_f2bdd9ee-838b-49ab-87da-f10da0a094fe
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-26T11:28:30.889575Z
---
One question, one predicate — and "which languages are code" had THREE copies. `is_code_lang` existed privately in kenn-indexer/src/atlas/producer.rs, publicly in atlas/domains.rs, and was imported by the packages query, which then failed to apply it in the place that mattered. Result: `kenn packages` counted anchors holding only markdown/html/css nodes as PACKAGES, while the atlas routed those same anchors to the documents axis. 4 vs 3 packages on one repo, 128 vs 125 on a 125-package solution. The producer now imports the shared one; the query filters its anchor map with it.

The subtler half: the packages query had TWO anchor maps with DIFFERENT filters — the one feeding central symbols applied `is_code_lang`, the one feeding the package LIST and the coupling pair weights did not. When one function builds two projections of the same concept, diff their filters; a predicate applied in one and not the other is invisible in review and shows up as a count mismatch on a big repo.

Second finding, same shape: a package name had two spellings. `parse_package_json_name` stripped `@scope/` for readability, so a symbol the TypeScript producer attributed carried the full `@nestjs/core` (via pkg_id → packages row) while a symbol without a pkg_id (a markdown doc) fell to the layout marker and got `core`. Both anchors then lived in the graph and `kenn packages` listed 26 for a 17-package repo — nine bare-name duplicates. Stripping is also lossy: `@a/utils` and `@b/utils` collapse onto one `utils`. Keep the scope; concept ids already carry it (`packages/typescript_@acme_web`).

DIAGNOSTIC that found both: compare the query count against the atlas index HEADER per repo, across languages. Small repos agreed and hid it; only the 125-package solution and a monorepo with scoped names exposed the two bugs, and the extras were characterized by asking what LANGUAGES their nodes had — all-content meant the code filter was missing, bare-name duplicates meant two naming rules.