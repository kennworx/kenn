# Tasks

## 1. Dominance + structure detection

- [x] 1.1 In `build_concepts`, compute the **dominance** test (D1, O3 resolved):
      a single-dominant repo = the top anchor owns a strict majority of production
      nodes (`top_prod * 2 > total_prod`) — integer, deterministic, no
      `DOMINANT_FRACTION`/`FEW_ANCHORS` constants needed. Pure over `central_nodes`.
      ✓ Exercised by `dominant_structured_…` (dominant → subdivides) and
      `single_dominant_repo_…` (dominant → intra-package domain); balanced fixtures
      (`domain_inputs`) are not single-dominant.

## 2. Source sub-area derivation

- [x] 2.1 Package-root = longest common dir prefix of an anchor's symbol def-file
      paths (`common_dir_depth`). Sub-area = first segment beyond that depth
      (`subarea_of`), so the common `Source`/`src` wrapper is stripped automatically
      (D2). ✓ Exercised via `dominant_structured_…` (`src/Core`, `src/Features` →
      `Core`, `Features`).
- [x] 2.2 Group a dominant anchor's symbols into sub-areas (`build_components`);
      keep only those with ≥ `MIN_SUBAREA_SYMBOLS`, and only subdivide when ≥
      `MIN_SUBAREAS` qualify (else leave the package flat). ✓
      `dominant_package_with_one_qualifying_subarea_stays_flat`.
      **Mutation-check (§9)** ✓: weakening the `MIN_SUBAREAS` guard makes the lone
      qualifying sub-area sprout a component (verified red).

## 3. The `component` concept

- [x] 3.1 Added the `component` concept type + `parent` (package concept id) to the
      atlas model, its OKF rendering, and a **Components** section on the parent
      package doc (O1 resolved → `component`, not a parented `package`). ✓
      `dominant_structured_…` asserts the component's type/parent/resource/members;
      okf render tests cover conformance + determinism.

## 4. Wire decomposition into the producer

- [x] 4.1 In `build_concepts`, when the anchor is dominant and its sub-areas qualify,
      call `build_components` (extracted helper) and attach the component ids to the
      package concept; both orchestration paths share the producer. ✓
      `dominant_structured_…` (multi-dir → components) +
      `package_members_report_…`/`dominant_package_with_one_qualifying_subarea_…`
      (flat/single-subarea → none).

## 5. Intra-package domains

- [x] 5.1 Relaxed `build_domains` (D4): a community qualifies when it clears
      `MIN_DOMAIN_SIZE` AND (`cross_anchor` OR the repo is single-dominant); the
      single-package-after-filtering drop is skipped only when single-dominant, so
      multi-package repos are unaffected. ✓ `single_dominant_repo_forms_an_intra_package_domain`
      (forms) + `single_package_community_is_not_a_domain` /
      `cross_anchor_flag_but_single_package_…` (balanced → dropped).
      **Mutation-check (§9)** ✓: dropping the `|| single_dominant` relaxation drops
      the intra-package domain (verified red).

## 6. Example / sample suppression

- [x] 6.1 Exclude symbols under an `example`/`sample`/`demo`/`fixtures` path
      segment (case-insensitive, full segment) from `domain_eligible` and central
      lists — one `continue` in the eligibility loop — while still counting them in
      member/symbol totals (D5, O2: built-in list). ✓
      `example_code_neither_fabricates_a_domain_nor_appears_central`.
      **Mutation-check (§9)** ✓: neutering the `continue` re-admits the demo →
      spurious cross-anchor domain forms + demo becomes central (verified red).

## 7. Package members: total + per-directory counts

- [x] 7.0 Replace the capped package member list with a `## Files under <package>`
      structural summary (D7): carry a `file_count` total + a `dir_counts` (parent
      dir → file count, sorted count-desc then path) on `Concept`, dropping the
      `MAX_MEMBERS` cap for the package path; render the heading total + one line
      per directory in `okf.rs`. A `component` still lists its individual files.
      Verify: a flat-package fixture → heading total + a single `src` line; a
      multi-dir fixture → per-dir counts summing to the total.
      **Mutation-check (§9)**: reinstate the `MAX_MEMBERS` cap and confirm an
      8-file package's total under-reports / a directory's count is wrong. ✓ done.

## 8. Fixtures, determinism, gates

- [x] 8.1 Two-directional fixtures: a dominant multi-dir anchor subdivides +
      surfaces intra-package domains (`dominant_structured_…`,
      `single_dominant_repo_…`); balanced many-anchor fixtures gain no components and
      only cross-anchor domains (`groups_by_anchor_…`, `single_package_community_…`,
      `cross_anchor_flag_but_single_package_…`). Determinism re-run covered by the
      integration tests `domain_bundle_is_deterministic` +
      `determinism_same_corpus_twice_byte_identical`.
- [x] 8.2 End-to-end: reindexed Alamofire (real single-package Swift library) —
      subdivided into `Core`/`Features`/`Extensions` components + intra-package
      domains, with the example-app symbols suppressed (no `URLEncoding`-via-example
      domain). Live-verified in-session; not an automated test (needs the Swift
      toolchain).
- [x] 8.3 Gates: `cargo clippy --workspace --all-targets`, `just crap-ci`,
      `cargo fmt --all` — all green.
