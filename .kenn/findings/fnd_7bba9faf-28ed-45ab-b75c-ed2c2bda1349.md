---
id: fnd_7bba9faf-28ed-45ab-b75c-ed2c2bda1349
tags:
- directive
- guide
parent_ids: []
created_at: 2026-08-12T14:59:30.610909Z
---
An openspec requirement body must carry SHALL or MUST on its FIRST line. The validator reads only the opening line, so a line-wrap that pushes SHALL to line two fails with 'must contain SHALL or MUST' even though the paragraph plainly contains it. Rewrap the sentence rather than hunting for a missing keyword.