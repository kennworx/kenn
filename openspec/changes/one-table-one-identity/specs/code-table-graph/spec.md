## ADDED Requirements

### Requirement: One table has one identity, however its references spell it

References that name the same table SHALL reach one identity whether or not they state its schema, so a table's references are one set rather than two halves.

A bare name means *schema unstated*, not *schema empty*. A qualified reference
SHALL therefore adopt an existing unqualified identity of the same name rather
than mint a sibling beside it. This is the rule `normalize_table_name` already
applies to quoting and dotted spellings, extended to the one axis it did not
cover.

The rule SHALL be asymmetric. Two *qualified* identities of the same name SHALL
NOT merge: two schemas can each hold an `events`, and collapsing them would be a
worse error than splitting one table — it would report references to a table that
never received them.

#### Scenario: A qualified reference adopts the bare identity

- **GIVEN** a workspace where an attribute declares `orders` with no schema
- **AND** a statement elsewhere references `sales.orders`
- **WHEN** the passes resolve
- **THEN** one identity carries both references
- **AND** it reports the declaration

#### Scenario: Two schemas keep two tables

- **GIVEN** statements naming `sales.orders` and `archive.orders`
- **WHEN** the passes resolve
- **THEN** two identities remain, each with its own references

#### Scenario: Adding a reference does not re-attribute a table

- **GIVEN** a table whose declaration is visible
- **WHEN** a later change lets one more reference to that table be seen
- **THEN** the table still reports its declaration
- **AND** its identity is unchanged

### Requirement: A reference never targets an identity that was never minted

The identity a reference carries SHALL be the identity that gets minted for it, so no reference can resolve to a node the run never wrote.

Today the mint guard tests the bare *name* while the reference carries the whole
key. One spelling of a table therefore satisfies the guard for the other, whose
edge then finds no target and is dropped. What is lost is not a near-duplicate: on
a real corpus this discarded a `createTable` declaration and left the table
reporting one reference of four.

Order decides which spelling survives, so the loss depends on the order files are
walked — the same workspace can report different references from one run to the
next as unrelated files are added.

#### Scenario: Either order yields the same references

- **GIVEN** a workspace where one table is named both bare and schema-qualified
- **WHEN** the passes resolve, in either order of first sighting
- **THEN** the table reports the same references both times

### Requirement: A dropped reference is counted, not silent

When a reference cannot be attributed to any table node, the run SHALL count it and report the count alongside its other producer diagnostics.

Skipping such a reference rather than failing the run is the right behaviour —
one missing edge should not cost a whole index. Being unable to *observe* it is
not: a silent skip is what let a lost declaration survive a full corpus run,
every unit test, and a green gate, and it was found only by diffing two indexes
by hand.

#### Scenario: An unattributable reference surfaces in the report

- **GIVEN** a reference whose identity no node was written for
- **WHEN** the run completes
- **THEN** the run reports how many references were dropped
- **AND** the count is zero for a workspace where every identity was minted
