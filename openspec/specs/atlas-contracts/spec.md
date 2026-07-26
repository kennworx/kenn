# atlas-contracts Specification

## Purpose
TBD - created by archiving change add-atlas-contracts-axis. Update Purpose after archive.
## Requirements
### Requirement: the contracts axis is derived from is-a edges

The atlas SHALL emit a **contracts** axis: one `contract` concept per first-party
interface, base class, or protocol whose implementers span more than one package.
The producer SHALL derive each contract directly from the aggregate `implements`
and `extends_type` edges (directed implementer → contract) and SHALL NOT use
Louvain clustering or the persisted analysis tables. The derivation SHALL be a
pure projection of `aggregate_nodes` + `aggregate_edges` at render time, requiring
no analysis pass, no reindex invalidation, and no `kenn index --force` to adopt.

#### Scenario: a cross-package interface becomes a contract

- **WHEN** a first-party interface defined in package A is implemented by
  first-party types in packages B and C (via `implements`/`extends_type` edges)
- **THEN** a `contract` concept is written for that interface, listing every such
  implementer grouped by its package

#### Scenario: contracts do not need the analysis pass

- **WHEN** a repo is indexed with the clustering/analysis pass disabled
- **THEN** the contracts axis is still emitted (it reads edges, not communities)

### Requirement: a contract lists every first-party non-test implementer

A contract concept SHALL list ALL first-party implementers of the contract — not
a subset — grouped by the implementer's package. Both the contract and each
implementer SHALL be first-party and anchored; a type implementing a vendored /
external interface SHALL NOT produce a contract, and an external implementer SHALL
NOT be listed. Test nodes SHALL be excluded on both ends: a test double of a
production contract is not part of its architecture.

#### Scenario: completeness

- **WHEN** an interface has three first-party implementer packages
- **THEN** the contract's package span is 3 and all three are listed (unlike a
  clustered domain, which may merge only some)

#### Scenario: test implementers are excluded

- **WHEN** an interface is implemented by two production types and one test double
- **THEN** the contract counts two implementers and omits the test double

### Requirement: only cross-package contracts earn a concept

A contract concept SHALL be written only when the contract's implementers span at
least two distinct packages. An interface whose implementers all live in one
package SHALL NOT produce a contract concept — that is local detail the package
concept already covers.

#### Scenario: a single-package interface is not a contract

- **WHEN** an interface and all its implementers live in the same package
- **THEN** no contract concept is written for it

### Requirement: contracts are ranked by breadth and every cap is named

The contracts axis SHALL be ordered with the broadest extension points first —
by package span descending, then implementer count, then name — and SHALL be
bounded by caps on the number of contracts, packages per contract, and
implementer names per package. Whenever a cap truncates output, the rendered
document SHALL state the full pre-cap count rather than truncate silently: the
implementers heading SHALL carry the total implementer and package counts, and a
truncated per-package cell SHALL name how many implementers it omits. The bundle
SHALL remain deterministic across re-index of unchanged code.

#### Scenario: a broad contract names what it dropped

- **WHEN** a contract has more implementer packages (or per-package implementers)
  than the render cap
- **THEN** the heading states the full `<N> across <M> packages` breadth and each
  truncated cell shows the shown names followed by a `… (+K)` remainder count

### Requirement: a contract concept renders as OKF with resolvable implementers

Each `contract` concept SHALL render with `type: contract`, the contract's kind
(e.g. `interface`) on the standard `tags` field, no `resource`, a link back to
the package the contract is **defined in**, and the contract type's own `pub_id`
and definition location (so the interface itself is `kenn get`-able and
jump-to-source). Implementers SHALL be grouped by
package into one section per package (a bundle-relative package link plus that
package's full implementer count), each section an `ID | Location` table where
every implementer carries its stable `pub_id` (usable with `kenn get <pub_id>`)
and workspace-relative source location — the same actionable shape a package
concept uses for its central symbols. The `index.md` header SHALL list the axis
under a `## Contracts` section, one line per contract with its total implementer
and package-span counts.

#### Scenario: an implementer is actionable

- **WHEN** a contract lists an implementer
- **THEN** the row carries the implementer's `pub_id` and `path:line` location, so
  a reader can `kenn get` it or jump to its definition

#### Scenario: index lists the contracts axis

- **WHEN** the bundle contains at least one contract
- **THEN** `index.md` has a `## Contracts` section listing each contract with its
  `<N> implementers across <M> packages`, and a `contracts/<slug>.md` document
  exists for it

