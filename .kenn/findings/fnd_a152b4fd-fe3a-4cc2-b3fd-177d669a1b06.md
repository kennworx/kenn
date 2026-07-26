---
id: fnd_a152b4fd-fe3a-4cc2-b3fd-177d669a1b06
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-07-25T07:40:54.590764Z
---
Flat community detection clusters a FIRST-PARTY, is-a-reweighted view of the graph (AggregatedGraph::clustering_view in kenn-analyze/src/projection.rs), not the raw graph: external nodes (and edges incident to them) are excluded because the atlas maps project code — clustering over vendored/stdlib types groups first-party symbols by what they both mention, not by their own structure — and the is-a family (implements/overrides/extends_type) is up-weighted x4 (cluster_kind_weight) so a contract bond outpulls incidental calls, which the aggregate weight ranks the other way. God-node ranking and the anchored hierarchy keep the FULL graph on purpose. Amplifying the is-a weight past x4 saturates (measured across six languages) — it is an on/off, not a dial. Capping per-pair edge occurrence to tame multiplicity was measured and rejected: it fragments clustering and does not fix breadth-driven hubs.