## Why

The atlas emits four axes — packages, domains, contracts, documents — but only
**packages** is reachable from the CLI or MCP. `DomainConcept` and
`ContractConcept` exist solely inside `kenn-indexer/src/atlas/`: computed at
index time, rendered to markdown, discarded. An agent that asks "which
interfaces are implemented across packages" or "what cross-package clusters
exist" has to read files, which is the workflow the query surface exists to
replace.

Worse than absent — **inconsistent**. `kenn overview` reports
`cross_anchor_communities: 38` while the atlas renders **9** domains for the
same snapshot. The 38 is the raw Louvain count; the 9 are the earned-span
domains that survive the member floor and the cross-package-link floor. The
atlas applies those floors precisely because raw communities overstate: they
include packages joined only through a shared vendor type, plus one-symbol
stragglers. So the CLI currently publishes the number the atlas was changed to
stop publishing, and a reader cannot tell which surface lied.

The data is not the obstacle. Both axes are derivable from a published snapshot
(`aggregate_nodes`/`aggregate_edges` plus the persisted analysis) — the same
inputs `list_packages` already reads. Only the **rules** are trapped in the
producer, which is exactly the failure mode `atlas/coupling.rs` was extracted to
prevent.

## What Changes

- **New MCP tools + CLI verbs** mirroring the missing axes:
  - `list_domains` / `kenn domains` — cross-package clusters, earned-span only.
  - `list_contracts` / `kenn contracts` — first-party interfaces / base types
    whose implementers span more than one package.
  - `list_documents` / `kenn documents` — first-party non-code directories.
- **`list_packages` / `kenn packages` completed** against the package concept it
  mirrors: the package's own root-module doc (`description`), its manifest path
  (`resource`), member-file count and per-directory counts, and its component
  sub-areas. Today the CLI drops all of these; `description` is the only
  authored prose in the atlas and is invisible to the query surface.
- **Domain and contract rules extracted** into shared modules
  (`atlas/domains.rs`, `atlas/contracts.rs`) consumed by BOTH the producer and
  the new queries — same pattern, and same rationale, as `atlas/coupling.rs`.
  Thresholds (`MIN_PKG_MEMBERS`, `MIN_DOMAIN_LINKS`, `MIN_CONTRACT_PKGS`, the
  render caps) get exactly one definition.
- **Axis listings paginate** rather than inheriting the atlas's render caps. A
  cap is presentation policy for a page with a reader; a query that silently
  returns the top 24 of 60 contracts re-commits the truncation defect the
  coupling tables were fixed to stop.

The `cross_anchor_communities` inconsistency that motivated this investigation is
NOT fixed here. Making the overview honest requires an earned-domain count in the
build-time `stats` table, and neither producer can write one today: `kenn-analyze`
owns the stats rows but has no edges to apply the earned-span rule, `kenn-indexer`
owns the rule but is optional in the pipeline, and the two crates do not depend on
each other. That is a crate-topology decision with its own blast radius, so it is
filed separately as `honest-graph-counters`. Nothing in this change depends on it.

## Capabilities

### New Capabilities

- `atlas-axis-queries`: querying the atlas's domain, contract, and document axes
  from a published snapshot — the axis rules (floors, caps, ranking, dedupe)
  live in one shared implementation that the atlas producer and the query
  surface both consume, so the two can never report different numbers for the
  same snapshot.

### Modified Capabilities

- `mcp-server`: adds the `list_domains`, `list_contracts`, `list_documents`
  read tools; extends `list_packages` with the package concept's remaining
  fields.
- `cli-query-surface`: the mirrored `domains`, `contracts`, `documents` verbs,
  and the flat-row shape they must emit so the default TOON output stays a
  header-once table rather than falling back to JSON.

## Impact

- **New code**: `crates/kenn-indexer/src/atlas/{domains,contracts}.rs` (rules
  extracted out of `producer.rs`); `crates/kenn-mcp/src/tools/{domains,contracts,
  documents}.rs`; CLI wiring in `crates/kenn-cli/src/cmd_query.rs` + `main.rs`.
- **Modified**: `atlas/producer.rs` (calls the extracted rules instead of owning
  them — behavior-preserving, pinned by the existing atlas tests);
  `kenn-mcp/src/tools/packages.rs` (`PackageView` gains fields).
- **Output-shape constraint**: the new view types must be FLAT for the bare
  listing (nested rows make TOON fall back to JSON), so nested detail —
  a domain's spanned packages, a contract's per-package implementers — is
  returned only when a single domain/contract is named. Same asymmetry
  `list_packages` already uses, and for the same reason: emitting every
  entity's nested detail is quadratic and unreadable at scale.
- **Determinism**: the queries read the aggregate graph and the persisted
  analysis; they never recluster. Flat Louvain's ordering contract already
  guarantees a stable partition across re-index, so a query answered from a
  snapshot matches the markdown built from the same snapshot.
- **No schema change.** No new tables; nothing is persisted that is not already
  persisted.
