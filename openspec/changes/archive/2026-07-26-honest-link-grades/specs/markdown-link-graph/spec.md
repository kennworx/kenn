## MODIFIED Requirements

### Requirement: Recall-first, name-anchored link resolution

Link resolution SHALL be name-anchored and qualifier-tolerant, downgrading
through a ladder rather than failing on a stale path or qualifier. The order
SHALL be: exact (path and name current) → drifted (name current, path/qualifier
stale) → fuzzy (approximate name) → ambiguous (multiple name matches) → dangling
(no name match). A resolved edge SHALL carry a match-quality grade reusing the
existing `match_kind` vocabulary (exact / prefix / case-insensitive / fuzzy).
Resolution SHALL NOT silently drop a link.

An inline link target is written relative to the **linking** file. Resolution
SHALL therefore accept a target either as written (already workspace-relative)
or joined onto the linking file's directory, and SHALL grade both `exact`.
Joining SHALL resolve `.` and `..` by popping path segments, and a target whose
`..` segments walk above the workspace root SHALL NOT resolve to any
in-workspace candidate. This SHALL be one rule for every link target — markdown
document, code file, or code symbol — so that one written link cannot be graded
two ways.

#### Scenario: A correct relative path to a code file resolves as exact

- **WHEN** `indexers/kenn-dotnet/README.md` links `[t](../frames.ts)` and the
  file exists at `indexers/frames.ts`
- **THEN** the written target is joined onto the linking file's directory,
  yielding `indexers/frames.ts`
- **AND** the link resolves to that file and the edge is graded **exact**
- **AND** it is NOT graded drifted merely because the written target and the
  candidate's path differ as strings

#### Scenario: A bare sibling name resolves by the join, not by locality

- **WHEN** `api/docs.md` links `[t](order.rs)` and files exist at both
  `api/order.rs` and `ui/order.rs`
- **THEN** the link resolves to `api/order.rs` and is graded **exact**
- **AND** the choice follows from the join, not from a locality tie-break

#### Scenario: A relative path is not graded exact against a same-named file elsewhere

- **WHEN** `crates/a/src/m/README.md` links `[t](../../x/mod.rs)`, no file exists
  at `crates/a/x/mod.rs`, and a different file does exist at `x/mod.rs`
- **THEN** the link SHALL NOT be graded exact against `x/mod.rs`
- **AND** resolution continues down the ladder (basename plus locality)

#### Scenario: A stale relative path resolves as drift

- **WHEN** `a.md` links `[t](../old/order.md)` but the file now lives at
  `notes/order.md`
- **THEN** the link resolves to `notes/order.md` by basename
- **AND** the edge is graded as drifted and recorded for reporting

#### Scenario: A stale namespace on a symbol link resolves as drift

- **WHEN** a note references symbol `Auth.OrderHandler` but the symbol is now
  `Billing.OrderHandler`
- **THEN** the link resolves by short name `OrderHandler`
- **AND** the edge is graded as drifted

#### Scenario: An approximate name resolves as fuzzy

- **WHEN** a note links `[[OrderHandlr]]` (a typo) and the only near match is
  `OrderHandler`
- **THEN** the link resolves to `OrderHandler`
- **AND** the edge is graded as fuzzy (low confidence) and recorded for
  reporting

#### Scenario: No name match dangles to an external stub

- **WHEN** a note links `[[gone]]` and no file, alias, title, or symbol matches,
  and nothing exists in the workspace at that path
- **THEN** an edge to an unresolved external node is emitted and reported as
  dangling

### Requirement: Link-health reporting

kenn SHALL expose a link-health report (`check_links`) that lists drifted,
fuzzy, ambiguous, and dangling links, each with both the written target and the
resolved target (where one exists). A link whose target exists in the workspace
SHALL NOT be reported as dangling, and the report SHALL NOT describe a working
link as broken.

#### Scenario: Drifted and dangling links are surfaced

- **WHEN** a corpus contains a drifted link and a dangling link
- **THEN** `check_links` lists the drifted link with its written and resolved
  targets, and lists the dangling link with no resolved target

#### Scenario: A link to an existing but non-indexed target is not reported

- **WHEN** a document links `[MIT](LICENSE-MIT)` or `[t](docs/)` and that target
  exists in the workspace
- **THEN** the link does not appear in the link-health report at all
- **AND** it is not counted toward the report's total

## ADDED Requirements

### Requirement: Unresolved markdown targets that exist become path-keyed attachments

The indexer SHALL check a markdown link target for existence in the workspace
before treating it as broken, whenever that target resolved to no markdown
document, no code file, and no code symbol.

A target that **exists** SHALL resolve to an `attachment` stub node
keyed by its **canonical workspace-relative path** — the written target joined
onto the linking file's directory and normalized — and the edge SHALL be graded
`exact`. A target that does **not** exist SHALL keep a stub keyed by the written
string and SHALL be graded `dangling`.

This is the same resolution HTML already performs for asset references, and the
canonical path SHALL be computed by the same rule, so that every markdown
reference to one on-disk target collapses to a **single** node. The two corpora
key their stubs in their own id namespaces (`md:@attachment/…` and `html:…`), so
a target referenced from both markdown and HTML yields one node per corpus, not
one overall; unifying that is a separate change to HTML's shipped stub ids.
Existence SHALL be supplied to the resolver as an injected lookup rather than
performed inside it, preserving the resolver's filesystem-free property.

Eligibility SHALL be decided by existence, not by spelling: a target with no
file extension, and a target naming a directory, are eligible on the same terms
as one carrying a known asset extension.

#### Scenario: An extensionless repository file resolves to an attachment

- **WHEN** `README.md` links `[MIT](LICENSE-MIT)` and that file exists at the
  workspace root
- **THEN** the edge resolves to an `attachment` stub keyed by `LICENSE-MIT`
- **AND** the edge is graded `exact`
- **AND** this holds even though the target carries no file extension, so
  eligibility cannot be decided by spelling

#### Scenario: A bare inline destination prefers an existing path over a symbol

- **WHEN** `README.md` links `[t](helper)`, a code symbol named `helper` exists,
  and the workspace also holds a path named `helper`
- **THEN** the edge targets the path, not the symbol
- **AND** this follows from the destination being an inline `CommonMark` link,
  whose destination denotes a path

#### Scenario: A wikilink prefers the symbol over a same-named path

- **WHEN** a document links `[[helper]]`, a code symbol named `helper` exists,
  and the workspace also holds a path named `helper`
- **THEN** the edge targets the code symbol
- **AND** the path preference SHALL NOT apply, because a wikilink denotes a
  name rather than a path

#### Scenario: A path-shaped target still resolves to its indexed file node

- **WHEN** a document links `[t](src/order.rs)` and that file is indexed
- **THEN** the edge is a `links_to_file` edge to the file node
- **AND** the path preference SHALL NOT divert it to an attachment, which
  applies only to bare names

#### Scenario: A directory reference resolves to an attachment

- **WHEN** `README.md` links `` [`docs/`](docs/) `` and that directory exists
- **THEN** the edge resolves to an `attachment` stub keyed by `docs`
- **AND** the edge is graded `exact`

#### Scenario: Different spellings of one target collapse to one node

- **WHEN** `README.md` links `[a](LICENSE-MIT)` and
  `crates/kenn-indexer/README.md` links `[b](../../LICENSE-MIT)`
- **THEN** both edges target the **same** attachment stub node
- **AND** `list_usages` on that node returns both references

#### Scenario: A target that does not exist still dangles

- **WHEN** a document links `[t](missing-file)` and nothing exists at that path
- **THEN** the edge targets a stub keyed by the written string
- **AND** the edge is graded `dangling`

#### Scenario: An indexed source file resolves to its file node, not an attachment

- **WHEN** a document links a path that resolves to an indexed code file
- **THEN** the edge is a `links_to_file` edge to that file node
- **AND** no attachment stub is minted for it, even though the file also exists
  on disk

#### Scenario: A target outside the workspace dangles

- **WHEN** a document links a path whose `..` segments walk above the workspace
  root
- **THEN** the target SHALL NOT resolve to an attachment
- **AND** the edge is graded `dangling`
