## 1. Extract the axis rules (behavior-preserving)

- [x] 1.1 Capture the baseline: re-index kenn's own repo and copy `.kenn/atlas/domains/` + `.kenn/atlas/contracts/` aside, so step 1.5 can prove the extraction changed nothing.
- [x] 1.2 Create `atlas/domains.rs`: move the floors (`MIN_DOMAIN_SIZE`, `MIN_PKG_MEMBERS`, `MIN_DOMAIN_LINKS`), the `supported_span` / `decide_span` / `community_pair_links` logic, and the intra-domain-degree hub ranking out of `producer.rs`. Inputs are input-agnostic projections (anchor names, node ids, weights), never `*Record` or `*Row` — follow `atlas/coupling.rs`.
- [x] 1.3 Create `atlas/contracts.rs`: move the caps (`MAX_CONTRACTS`, `MAX_CONTRACT_PKGS`, `MAX_IMPLEMENTERS_PER_PKG`), the `MIN_CONTRACT_PKGS` floor, the first-party/non-test filtering, and `dedupe_contract_ids`, with the same input-agnostic shape.
- [x] 1.4 Rewrite `producer.rs`'s `build_domains` / `build_contracts` as projections into the shared modules; delete the moved constants so exactly one definition survives.
- [x] 1.5 Verify: existing atlas tests pass unchanged, and a re-index produces a byte-identical domains + contracts bundle vs the 1.1 baseline (`diff -r`). Any difference is a bug in the extraction, not an improvement.
      **Note:** kenn's OWN repo cannot validate this — indexing it re-reads the source this change edits, so the graph legitimately moves. Verified instead on unchanged external repos with old-vs-new binaries: Go byte-identical (domains + contracts); Swift contracts byte-identical. Swift *domains* differ, but the OLD binary differs from ITSELF across two runs on the same 5 files, so that is pre-existing symbol-id non-determinism, not a regression here.

## 2. Domains query

- [x] 2.1 Add `DomainView` (flat scalars: `id`, `title`, `size`, `packages_count`, `links`) and the nested detail types, `id` first per the CLI output directive.
- [x] 2.2 Implement `list_domains` in `kenn-mcp/src/tools/domains.rs`: project snapshot rows + persisted analysis into the shared module's inputs; bare returns flat rows, a named domain adds spanned packages and central symbols. Keep the pure computation separate from the async I/O shell so it is testable without a published snapshot.
- [x] 2.3 Test: earned-span only (a community joined solely through a shared external type is absent — mutation-checked by removing the link floor); empty list, not an error, when no cross-package clusters exist.

## 3. Contracts query

- [x] 3.1 Add `ContractView` (flat scalars: `symbol`, `title`, `kind`, `defined_in`, `implementers_count`, `package_span`) plus nested per-package implementer detail.
- [x] 3.2 Implement `list_contracts` in `kenn-mcp/src/tools/contracts.rs` over the shared module, reading `implements`/`extends_type` from the aggregate edges. Report pre-cap totals so a capped response never reads as complete.
- [x] 3.3 Test: single-package interfaces excluded; test and external implementers excluded; empty axis returns an empty list (Rust/Go legitimately have none).
- [x] 3.4 Resolve the name argument as a QUERY (title or `pub_id`), returning every match grouped and tagged with its `pub_id` when a title is ambiguous — never an error, never a second roundtrip. Test with two same-named types in different packages; mutation-check that the ambiguous case does not collapse to one match.

## 4. Documents query

- [x] 4.1 Add `DocumentView` (`id`, `title`, `path`, `file_count`) and implement `list_documents`; no file contents.
- [x] 4.2 Test: first-party non-code directories are listed with file counts.
- [x] 4.3 Wire `kenn documents` as a subcommand-capable group (the `find` pattern — `sub: Option<…>` with a bare default listing) so future subcommands/flags have a home without reshaping the verb. Do NOT add a `--documents` flag to `kenn packages`.

## 5. Parity guard

- [x] 5.1 Add a parity test that runs the producer path and the query path against ONE fixture graph and asserts identical domain ids/titles/sizes and contract ids/kinds/spans. This is the test whose absence let the overview drift go unnoticed — it must fail if either surface is changed alone.

## 6. Complete the packages query

- [x] 6.1 Extend `PackageView` with `description` (root-module doc, verbatim, omitted when absent) and `resource` (package root).
      **Reduced scope:** `file_count` / `dir_counts` / `components` are NOT reported. The atlas counts the files of every symbol in the package, mapped through the aggregation rollup (`sym_anchor` walks `aggregate_of`); a snapshot query sees only aggregate ROOTS, so counting from them undershoots (57 vs the atlas's 73) and counting every def row overshoots (86). Reproducing the rollup on the read path is disproportionate, and a count that disagrees with the document is the exact defect this change exists to remove — so the fields are omitted rather than approximated.
- [x] 6.2 Keep the row flat for the bare listing — `dir_counts` and components are nested, so they belong to the named-package response only, or the TOON table falls back to JSON.
- [x] 6.3 Test: a documented package carries its doc verbatim; an undocumented one omits the field and nothing is synthesized.

## 7. Pagination

- [x] 7.1 Give each axis tool a `Pagination` argument and a `next` cursor, like every other listing tool. Render caps stay atlas-side and MUST NOT bound a query (design D8).
- [x] 7.2 Expose `--page-size`, `--cursor`, and `--all` on the new verbs, per the existing universal-flags requirement; `--all` drains and preserves trailing metadata.
- [x] 7.3 Test: an axis larger than one page is drainable with every entity returned exactly once; a bounded response reports its pre-cap total.

## 8. CLI mirroring

- [x] 8.1 Add `kenn domains [<query>]`, `kenn contracts [<query>]`, `kenn documents` to `cmd_query.rs` + `main.rs`, each a thin wrapper emitting its own typed result via the generic `emit` (no type-erased carrier, no `serde_json::Value`).
- [x] 8.2 Verify each bare verb renders as a TOON table with the id leading, and each named form adds nested detail (which correctly falls back to JSON when nested).
- [x] 8.3 Update the `kenn` skill / agent-facing docs so the new verbs are discoverable, using user-facing terms only.

## 9. Gates

- [x] 9.1 `cargo clippy --workspace --all-targets` clean (pedantic).
- [x] 9.2 `just crap-ci` passes; split any new over-threshold function rather than baselining it.
- [x] 9.3 Run the new verbs against a real multi-language repo and sanity-check against the rendered atlas.
      **Result:** domains 4/4, contracts 0/0, documents 1/1 — exact. Cost answered: the domains query is **0.05s** (the analysis-table read D5 flagged as unmeasured is cheap); `packages` is 1.2s, the slowest, from its four scans.
      **Found (PRE-EXISTING, not this change):** `kenn packages` reports a root anchor the atlas omits (4 vs 3 packages). The OLD binary does the same, so it predates this work — the package axis has the producer-vs-query eligibility divergence that domains just had fixed. Filed as follow-up.
- [x] 9.4 `cargo fmt --all` last.
