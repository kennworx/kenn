---
id: fnd_87bbde56-3faa-425e-a0ea-9d9b508c234b
tags:
- directive
- polarity:do
- supersedes:fnd_26a3d92d-79b3-444f-882c-8cd4bfb572c7
parent_ids:
- fnd_26a3d92d-79b3-444f-882c-8cd4bfb572c7
created_at: 2026-08-13T12:43:15.792276Z
---
kenn-model `Kind::db_name` is DERIVED (`strum::IntoStaticStr` + `serialize_all = "snake_case"`), not a hand-written 33-arm match, and crap-baseline.json carries ZERO exceptions. This supersedes the earlier rule that grandfathered it at cyclo=31 as "legit debt, do not contort into a table-driven scan". That rule was right to protect exhaustiveness and wrong to conclude the baseline was the only way out: the derive expands over every variant, so a new variant still cannot silently miss its name, while cyclomatic drops to 1. A table-driven SCAN would trade exhaustiveness away; a DERIVE does not. Do NOT reintroduce the match, and do NOT add baseline entries: the gate runs with an empty exception set, and an entry there can silently outlive the problem it recorded (the `embed_pending` entry named a function of cyclomatic 1, already fixed by an unrelated refactor, carrying phantom debt since). A variant-to-string enum small enough to pass the gate on its own keeps its match; reach for the derive only when the arm count IS the whole score.