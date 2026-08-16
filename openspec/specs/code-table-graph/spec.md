# code-table-graph Specification

## Purpose
TBD - created by archiving change code-table-references. Update Purpose after archive.
## Requirements
### Requirement: A code symbol that names a table references it

The system SHALL emit a table reference from a code symbol whose body contains SQL
naming a table, using the same edge kinds and the same canonical table nodes a `.sql`
statement produces.

A table's references SHALL therefore be one homogeneous set regardless of what carried
them. Asking which code touches a table is the question the SQL work exists to serve, and
it is unanswerable while migrations and application code point at different things or at
nothing.

Extraction SHALL go through the existing shared extractor. A second SQL parser for code
is forbidden by the extractor's own contract, and the two would drift on exactly the
dialect and normalization decisions that took measurement to settle.

Resolution SHALL go through the one table-registry trait, backed by the identity set read
from the store. Only an in-memory implementation exists today, serving the `.sql` pass;
this is the second consumer needing a store-backed one and `xml-sql-bridge` is the other.
Whichever lands first SHALL provide it and the other SHALL reuse it. Two store-backed
implementations of one lookup is the exact duplication the registry requirement was
written against, and this repository already carries that mistake once in its class
registry.

#### Scenario: A function that queries a table links to it

- **WHEN** a function's body contains a statement selecting from a table
- **THEN** that function emits an `AccessesTable` edge to it
- **AND** the edge points at the same node a `.sql` reference to that table reaches

#### Scenario: Schema DDL in code declares, as a migration does

- **WHEN** a code symbol's body contains statements creating tables
- **THEN** it emits `DefinesTable` edges for them
- **AND** those tables are marked as declared, exactly as a `.sql` declaration marks them

#### Scenario: One extractor serves code and files alike

- **WHEN** identical SQL text appears in a `.sql` file and in a code literal
- **THEN** the extracted references and grades are identical

### Requirement: A reference is attributed to the innermost symbol containing it

The system SHALL attribute each literal to the symbol whose stored body extent is the
smallest one containing it, and SHALL NOT attribute it to every symbol whose extent
contains it.

Body extents nest. A module's extent contains its functions', a class's contains its
methods', so attributing to every containing symbol gives an enclosing scope every table
its children touch. Measured on a self-index, a module received the full table set of the
function inside it — and at scale that answer degrades from "this function reads
`sessions`" to "this crate reads everything", which is no answer.

A literal outside every recorded extent SHALL contribute nothing. There is no symbol to
attribute it to, and attributing it to its file would reintroduce the same collapse one
level up.

#### Scenario: An enclosing scope does not inherit its children's tables

- **WHEN** a function inside a module contains a SQL literal
- **THEN** the function references the table
- **AND** the module does not

#### Scenario: The nearest of several nested scopes wins

- **WHEN** a method inside a class inside a module contains a SQL literal
- **THEN** only the method references the table

#### Scenario: A literal outside any symbol contributes nothing

- **WHEN** a SQL literal sits outside every recorded body extent
- **THEN** no reference is emitted for it

### Requirement: Literals are recovered from source, per language

The system SHALL recover string-literal contents by reading each file's source and
applying that language's literal syntax, and SHALL scan a file once rather than once per
symbol it contains.

Reading source is what makes this possible at all: the index carries symbols, ranges, and
roles, never literal values. The extents needed to place a literal are already stored,
having been captured for source retrieval.

Scanning per file rather than per symbol is not only an optimization. Nested extents mean
per-symbol slicing re-reads the same bytes once per enclosing scope, which is the shape
that produces the duplicate attribution the previous requirement forbids.

A language whose literal syntax is not implemented SHALL contribute no references and
SHALL NOT be reported as a failure. Absence of support is not a defect in the workspace.

#### Scenario: A language's own literal forms are recognized

- **WHEN** a file uses that language's raw or verbatim string form
- **THEN** the literal's contents are recovered as written
- **AND** its escape conventions do not corrupt the recovered text

#### Scenario: An unsupported language is silent

- **WHEN** a file's language has no literal scanner
- **THEN** it contributes no references
- **AND** the run reports no failure for it

### Requirement: Text that is not SQL is silent

The system SHALL treat a literal that does not parse as SQL as ordinary text, contributing nothing and reporting nothing.

Most literals in any codebase are messages, paths, formats, and keys. Measured on a
self-index, 4103 bodies carried literals and 154 yielded a table — so treating a
non-parse as a failure would report a defect on 97% of the corpus.

When a literal parses only in part, the system SHALL keep the references made by
statements whose verb names a schema object by grammatical position — `CREATE TABLE`,
`CREATE INDEX`, `ALTER TABLE`, `DROP TABLE` and their kin — and SHALL discard the rest.

A query's names cannot be trusted under a partial parse: an alias or a CTE
introduced by a torn-away clause reads as a table. A DDL statement's names can,
because the grammar admits nothing but a schema object in its target slot. The
distinction is a property of the grammar rather than a guess about codebases,
which is why it survives when the surrounding bytes did not parse.

Discarding the whole literal instead SHALL NOT be done. It is a bad trade whenever a
schema constant carries one statement the parser cannot read: measured on this
workspace's own `GRAPH_DDL`, a single `CREATE VIRTUAL TABLE … USING fts5(words,
tokenize='unicode61')` — which no supported dialect accepts — cost all 26 statements
around it, and with them every declaration of the 14 tables the constant creates.

#### Scenario: Ordinary literals produce nothing

- **WHEN** a body contains log messages, paths, and format strings
- **THEN** no reference is emitted
- **AND** no failure is reported

#### Scenario: A fragment is not a statement

- **WHEN** a literal holds part of a query rather than a whole one
- **AND** a split piece of it parses as a query naming a query-local name
- **THEN** it contributes no reference

#### Scenario: A schema constant survives one unreadable statement

- **GIVEN** a literal holding many `CREATE TABLE` and `CREATE INDEX` statements
- **AND** one statement using a vendor extension no supported dialect parses
- **WHEN** the pass reads it
- **THEN** every table the readable statements declare is referenced
- **AND** the unreadable statement contributes nothing
- **AND** the literal is not reported as a failure

### Requirement: Coverage is reported, not implied

The system SHALL report what this pass could and could not reach, so a reader can tell a
table nothing touches from a table whose access it could not see.

This finds SQL written as a literal. A concatenated query, a builder API, and an
ORM-mapped entity all access tables invisibly here, and an ORM-only codebase yields
nothing at all while being fully instrumented against its database. A silent zero would
read as "no code touches this", which is the wrong answer given confidently.

References from test code SHALL be emitted and marked, never dropped at index time. Test
symbols already carry a test flag and every query surface filters on it by default, so
emitting them costs nothing and dropping them would discard an answer the graph could
give — which tests exercise a table is a real question, and an index-time filter makes it
permanently unanswerable while diverging from every other producer.

#### Scenario: The run reports what it scanned and what it found

- **WHEN** the pass completes
- **THEN** its report distinguishes bodies scanned, bodies carrying literals, and
  references emitted

#### Scenario: A test-only reference is recorded and marked

- **WHEN** a table is referenced only from test code
- **THEN** the reference is emitted
- **AND** it carries the referencing symbol's test marking, so the existing query
  filters exclude it by default without the graph having lost it

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

