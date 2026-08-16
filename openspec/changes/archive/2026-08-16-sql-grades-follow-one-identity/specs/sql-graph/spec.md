## MODIFIED Requirements

### Requirement: Table references are graded, and only unknowable ones are dropped

Every table reference SHALL resolve to exactly one identity, and every table edge SHALL carry a `LinkGrade` recording how it resolved:

- `Exact` — the reference determines its identity. This is now every emitted edge:
  a qualified reference names its schema, and an unqualified one resolves by the
  rule below, so no surviving reference is uncertain about which node it points at.
- Dropped — the reference's target is **not knowable statically**, for example a
  name supplied by runtime substitution. Only this case produces no edge and no node.

An unqualified reference SHALL adopt the one schema that qualifies its name, and
SHALL stand for itself as an unqualified identity when two or more do.

`Ambiguous` is no longer produced for table references. It previously meant "this
unqualified name matches several known tables", and every match was kept as its
own edge. That does not invent a *table*, but it invents *references*: measured on
a real corpus, `transfers` reported 96 + 83 + 48 = 227 references where its sources
make 101, because each bare reference emitted an edge to both `wallets.transfers`
and `public.transfers`. A reader asking how much code touches `wallets.transfers`
was told 96 when 15 of them said so.

Adopting the single qualifying schema is safe for the opposite reason: one table
written two ways is the common case, and keeping its references in one set is what
makes the count true. Refusing to choose between two schemas is what keeps that
from becoming a guess.

Whether a name has one qualifying schema or several is a fact about the whole
workspace, so it SHALL be decided over the complete reference set rather than
incrementally as references are read. Deciding it incrementally makes a table's
identity depend on the order files are walked.

A reference SHALL NOT be dropped merely because nothing declares the table it names.
The dropped set is exactly the set of names the source does not determine — not the set
the workspace happens not to declare.

#### Scenario: An unqualified reference adopts the one schema that qualifies it

- **WHEN** `sql:analytics.users` is the only qualified identity named `users`
- **AND** a statement references bare `users`
- **THEN** one `AccessesTable` edge is emitted to `sql:analytics.users`, graded `Exact`
- **AND** its references are not split across two identities

#### Scenario: An unqualified reference refuses to choose between two schemas

- **WHEN** both `sql:wallets.transfers` and `sql:public.transfers` are known
- **AND** a statement references bare `transfers`
- **THEN** one `AccessesTable` edge is emitted to the unqualified identity `sql:transfers`
- **AND** no edge is emitted to either schema's table

#### Scenario: Identity does not depend on walk order

- **GIVEN** a workspace naming one table both bare and schema-qualified
- **WHEN** the workspace is indexed with its files walked in any order
- **THEN** the same identities are produced each time

#### Scenario: A runtime-substituted table name produces nothing

- **WHEN** a statement's table name is supplied by runtime substitution
- **THEN** no edge and no node are produced for it
