## MODIFIED Requirements

### Requirement: SQL carried in element text becomes table references

The system SHALL attempt to parse each XML element's own text as SQL, and SHALL emit the same table references a `.sql` statement carrying that text would — through the same shared extractor, with the same dialect recovery, and with the same grading.

References SHALL use the same edge kinds a `.sql` statement produces — `DefinesTable`
for text that brings a table into being, `AltersTable` for text that changes an existing
one, `AccessesTable` for text that reads or writes its data — so a table's references
are one homogeneous set regardless of which file carried them.

Element text is a single fragment, not a file of statements, so the whole-file-before-
splitting rule the SQL producer follows does not apply to it and SHALL NOT be imposed
here. Dialect recovery, identifier normalization, and grading all still apply.

When element text parses only in part, the system SHALL apply the same rule a code
literal does: keep the references made by statements whose verb names a schema object
by grammatical position, and discard the rest. A changelog's `<sql>` body is the same
kind of artifact as a schema constant in code, and which file it lives in SHALL NOT
decide whether its `CREATE TABLE` statements are seen.

Element text that does not parse as SQL SHALL contribute nothing and SHALL NOT be
reported as a failure. Most element text is prose, identifiers, or numbers; treating
non-SQL as an error would make the common case look broken.

Because the extractor is shared, text and `.sql` files cannot diverge in what they
consider a table, which dialects they accept, or how they grade a match.

#### Scenario: An element carrying a query links to its tables

- **WHEN** an element's text is a statement selecting from two tables
- **THEN** both tables are referenced with `AccessesTable`

#### Scenario: A changeset body survives one unreadable statement

- **GIVEN** an element whose text declares a table and also uses a vendor extension
  no supported dialect parses
- **WHEN** the bridge reads it
- **THEN** the declared table is referenced with `DefinesTable`
- **AND** the unreadable statement contributes nothing
