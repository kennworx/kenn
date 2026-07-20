---
id: fnd_1ac13ad1-867b-4dea-bc23-ad52f7be4952
tags:
- directive
- polarity:do
- edge-model
parent_ids: []
created_at: 2026-06-19T15:58:49.771993Z
---
kenn link/reference edge kind is chosen by the TARGET'S TABLE, not by source syntax. `LinksToFile` hydrates its target from the FILES table; `LinksTo`/`Embeds` hydrate from the SYMBOLS table. File and symbol ShortIds collide in one id space, so the edge kind is what tells the reader which table to read (the same trick `Contains` uses for module→file).

Consequence that's easy to get wrong: a reference to a NON-indexed asset (image/pdf/font/etc.) targets an `attachment` — a leaf STUB node in the SYMBOL space, because kenn does not index binaries — so it MUST be `LinksTo`, NOT `LinksToFile` (`LinksToFile` is only for references that resolve to an indexed file). When adding a reference edge for a new language: first resolve WHAT the target is (indexed file → `LinksToFile`/`Imports`; symbol/section/anchor/attachment-stub → `LinksTo`) and pick the edge from that, never from the syntax of the reference.