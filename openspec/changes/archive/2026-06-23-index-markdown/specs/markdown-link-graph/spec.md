## ADDED Requirements

### Requirement: Markdown links become graph edges

The indexer SHALL emit a `links_to` edge for each inline link `[t](target)`,
reference link, and wikilink `[[target]]` (including `[[target#anchor]]`,
`[[target|alias]]`, and same-file `[[#anchor]]`). Link targets MAY be a markdown
file, a markdown section, a code file, or a code symbol. External URLs SHALL NOT
produce graph edges. Backlinks SHALL be served by the existing usage-navigation
tools (inbound `links_to`).

#### Scenario: A wikilink produces a navigable backlink

- **WHEN** `a.md` links `[[b]]` and `b.md` exists
- **THEN** a `links_to` edge from `a.md` to `b.md` is present
- **AND** `list_callers` / `list_usages` on `b.md` returns `a.md`

#### Scenario: A link to a section resolves to the section node

- **WHEN** `a.md` links `[t](b.md#flow)` and `b.md` has a `## Flow` heading
- **THEN** the edge targets the `#flow` section node, not just the file

#### Scenario: An external URL is not a graph edge

- **WHEN** a note links `[t](https://example.com)`
- **THEN** no `links_to` graph edge is emitted for it

### Requirement: Transclusion is a distinct embed edge

The indexer SHALL emit an `embeds` edge (distinct from `links_to`) for each
transclusion `![[target]]` / `![[target#section]]`. The distinction SHALL be
preserved in the graph so that "what is inlined where" is queryable separately
from "what references this."

#### Scenario: Transclusion and reference are distinguishable

- **WHEN** `host.md` transcludes `![[note#highlights]]` and also links
  `[[note]]`
- **THEN** the transclusion is an `embeds` edge and the reference is a
  `links_to` edge, each separately queryable

### Requirement: Recall-first, name-anchored link resolution

Link resolution SHALL be name-anchored and qualifier-tolerant, downgrading
through a ladder rather than failing on a stale path or qualifier. The order
SHALL be: exact (path and name current) → drifted (name current, path/qualifier
stale) → fuzzy (approximate name) → ambiguous (multiple name matches) → dangling
(no name match). A resolved edge SHALL carry a match-quality grade reusing the
existing `match_kind` vocabulary (exact / prefix / case-insensitive / fuzzy).
Resolution SHALL NOT silently drop a link.

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

- **WHEN** a note links `[[gone]]` and no file, alias, title, or symbol matches
- **THEN** an edge to an unresolved external node is emitted and reported as
  broken

### Requirement: Ambiguous symbol links use locality then keep all

When a symbol link's short name matches multiple symbols, resolution SHALL
prefer the symbol nearest the linking markdown file by path distance. If
locality cannot disambiguate, resolution SHALL emit an edge to **every**
candidate (keep-all) and record the ambiguity.

#### Scenario: Locality breaks a tie

- **WHEN** `OrderHandler` exists in two crates and the linking note sits within
  one crate's subtree
- **THEN** the nearer `OrderHandler` is chosen

#### Scenario: Irreducible ambiguity keeps all candidates

- **WHEN** locality cannot distinguish multiple equally-near matches
- **THEN** an edge is emitted to each candidate
- **AND** the link is reported as ambiguous

### Requirement: Code-link resolution is gated to in-repo roots and the post-code barrier

Markdown-to-code resolution SHALL run only after all code ingest units complete,
and SHALL apply only to markdown roots inside the repository. External vault
roots SHALL resolve markdown-to-markdown links only; their code-looking
references SHALL remain text/unresolved.

#### Scenario: In-repo doc resolves to code, enabling code→md backlink

- **WHEN** an in-repo `docs/auth.md` references `OrderHandler`
- **THEN** after code ingest completes, a `links_to` edge to the code symbol is
  emitted
- **AND** `list_usages` on that code symbol returns the doc section

#### Scenario: External vault does not resolve into code

- **WHEN** a note in an external vault references `OrderHandler`
- **THEN** no markdown-to-code edge is emitted for it

### Requirement: Link-health reporting

kenn SHALL expose a link-health report (`check_links`) that lists drifted,
fuzzy, ambiguous, and broken links, each with both the written target and the
resolved target (where one exists).

#### Scenario: Drifted and broken links are surfaced

- **WHEN** a corpus contains a drifted link and a broken link
- **THEN** `check_links` lists the drifted link with its written and resolved
  targets, and lists the broken link with no resolved target
