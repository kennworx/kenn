---
id: fnd_1bef0f98-ebc7-4728-bfab-eb2388f1f9c5
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-06-18T11:50:47.869512Z
---
CSS usage mining (Phase 2, `uses_class` edges) emits an edge ONLY on a registry hit — never create a stub node or edge for a class-shaped token that has no matching CSS class. The markdown precedent dangles unresolved links to external stub nodes, but for CSS that would explode the graph: a Tailwind codebase would mint a stub + edge for every `flex`/`pt-4`/`mt-2` across every file (CSS's dangling case is the common case, not the rare one). Undefined class-shaped tokens are collected (with file+offset) into the `check_css` report only — gated on a utility allowlist — not materialized as graph nodes. The edge grade encodes match-confidence (Exact/Fuzzy/Ambiguous), not definedness. `usage_sources` defaults EMPTY (explicit opt-in: not every project in a repo uses CSS); with it unset, usage mining is off, orphan-class detection is skipped (else every class reports orphaned), and a one-time hint is emitted.