---
id: fnd_bded8bb9-ba15-4a6f-9d4f-e1ecc2bc0088
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-17T15:07:33.619971Z
---
Atlas aggregation (aggregate.rs walk_to_aggregate) must NOT let an EXTERNAL module/namespace be a symbol's aggregate root — gate module_fallback on `!row.external`. scip-python encodes a module's dotted path only in its members' descriptors and never emits the module as a defined symbol, so kenn's end-of-job stub-drain marks it external (def-less). Rolling first-party functions into that external container collapses them onto an `<unanchored>` node: the internal call edge becomes a dropped self-loop and the whole package degrades to a bare atlas `document` instead of a `package`. Only INTERNAL modules anchor their members. Rust is immune because rust-analyzer puts the crate in the SCIP head, so crate-root fns have enclosing_sym_id=0.