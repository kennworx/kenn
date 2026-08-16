## MODIFIED Requirements

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
