## ADDED Requirements

### Requirement: the atlas has a contracts axis

Alongside the package and domains axes, the bundle SHALL emit a **contracts**
axis — `contract` concept documents plus a `## Contracts` section in `index.md` —
whose behavior is specified by the `atlas-contracts` capability. The bundle SHALL
write one `contracts/<slug>.md` per contract and include the contracts axis in the
`index.md` concept count. The contracts axis SHALL be additive: the existing
packages and domains axes are unchanged, and a bundle with no cross-package
contract writes no `## Contracts` section and no `contracts/` files.

#### Scenario: the bundle carries three axes

- **WHEN** a multi-package repo with cross-package interfaces is indexed
- **THEN** the bundle contains package concepts, domain concepts, and contract
  concepts, and `index.md` lists `## Domains` and `## Contracts` sections

#### Scenario: no contracts, no section

- **WHEN** a repo has no first-party interface implemented across package
  boundaries
- **THEN** the bundle writes no `contracts/` files and `index.md` has no
  `## Contracts` section (packages and domains are unaffected)
