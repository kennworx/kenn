## 1. Model

- [x] 1.1 Add `ContractConcept` and `ContractImplementers` to `atlas/model.rs`
  (id, title, kind, defined-in package, per-package implementers, total + span
  counts).

## 2. Derivation (producer)

- [x] 2.1 Add `build_contracts(nodes, edges, node_anchor, anchor_lang)` in
  `atlas/producer.rs`: group `implements`/`extends_type` edges by contract (dst),
  both endpoints first-party + non-test, implementers grouped by package.
- [x] 2.2 Keep only contracts spanning ≥ `MIN_CONTRACT_PKGS` (2); rank by span
  desc, then implementer count, then name; apply `MAX_CONTRACTS`,
  `MAX_CONTRACT_PKGS`, `MAX_IMPLEMENTERS_PER_PKG` caps; dedupe slugged ids.
- [x] 2.3 Wire contracts into `build_concepts`' return and `write_bundle`; write
  `contracts/<slug>.md`; update the production caller in `aggregate.rs`.

## 3. Rendering (okf)

- [x] 3.1 Add `contract_id` (shared slug logic with `domain_id`) and
  `render_contract` (frontmatter `type: contract`, kind tag, defined-in link,
  implementers table).
- [x] 3.2 Name every cap: implementers heading carries full `<N> across <M>
  packages` breadth; a truncated cell shows `… (+K)`.
- [x] 3.3 Add the `## Contracts` section to `render_index` and count contracts in
  the concept total.

## 4. Tests (mutation-checked)

- [x] 4.1 `build_contracts`: cross-package interface becomes a contract; test
  implementer excluded; single-package interface produces none (mutation-checked).
- [x] 4.2 `render_contract`: OKF-conformant, defining-package link, capped heading
  + `… (+K)` cell, deterministic.

## 5. Validation on real repos

- [x] 5.1 Verify on public repos: Go `spf13/afero` (`File`, 4 packages) and Swift
  `apple/swift-argument-parser` (`ParsableCommand`, production conformers only),
  plus a large multi-package solution for the cap paths.

## 6. Gates

- [x] 6.1 `cargo clippy --workspace --all-targets` clean.
- [x] 6.2 `just crap-ci` passes (render_contract covered by 4.2).
- [x] 6.3 `cargo fmt --all`.

## 7. Docs

- [x] 7.1 Update the `atlas` skill (`claude-plugins/kenn/skills/atlas/SKILL.md`)
  to describe the contracts axis so agents read it — what it is (interface →
  implementers across packages), when to use it (before changing a shared
  abstraction / as an interface's blast radius), and how it differs from the
  package-coupling `implements` split.
- [x] 7.2 The `index.md` `## Contracts` section is the in-bundle guidance; the
  renderer emits it (task 3.3), so no separate overview edit is needed.
