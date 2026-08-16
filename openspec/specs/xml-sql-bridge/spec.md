# xml-sql-bridge Specification

## Purpose
TBD - created by archiving change xml-sql-bridge. Update Purpose after archive.
## Requirements
### Requirement: Cross-producer SQL resolution runs at the post-code barrier

The system SHALL resolve XML-carried SQL and table references in a barrier step that
runs after every producer has joined, reading XML element nodes and the table registry
from the building store rather than from deferred producer state.

Reading the store rather than carrying pending state keeps both contributing producers
barrier-free: each writes what it knows in one pass, and the join happens where every
input already exists. The step SHALL run after the XML and SQL producers and SHALL be
skipped without error when either contributed nothing. Because neither input depends on
code, the step MAY run as soon as both producers have joined rather than after the
markdown, stylesheet, and HTML steps.

This design depends on both stored inputs being lossless, and SHALL NOT be attempted
otherwise. A table's public id carries no path, so the identity set round-trips out of
`sql:<schema>.<name>` exactly; an element's signature is well-formed markup and its
content is its text verbatim, so both a configured attribute's value and a SQL statement
round-trip out of the store. A signature flattened into space-separated words does not
round-trip — `column name first name type varchar` cannot say where the value ends — and
the step would then need the producer to defer state, giving up the property this
requirement exists to preserve.

A failure in this step SHALL degrade its own report and SHALL NOT abort the run or
discard what the producers already wrote.

#### Scenario: The join sees both producers' output

- **WHEN** a workspace contains both `.sql` files and XML carrying SQL
- **THEN** the barrier step resolves references from both against one table identity set
- **AND** neither producer had to defer state to make that possible

#### Scenario: Nothing to join is not an error

- **WHEN** a workspace has XML but no SQL producer enabled, or the reverse
- **THEN** the barrier step is skipped
- **AND** the index run completes with the producers' own output intact

#### Scenario: A failed join keeps the producers' output

- **WHEN** the barrier step fails
- **THEN** the XML and SQL nodes written before it remain in the snapshot
- **AND** the step's report is degraded and the run completes

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

### Requirement: A workspace declares where its database XML lives

The system SHALL accept a list of roots scoping which XML this step considers, defaulting
to the whole workspace, and SHALL apply that scope to both the element-text arm and the
configured-attribute arm.

Scoping is a precision control before it is a cost control. Every element outside a
declared root is text a workspace never claimed was SQL, and trying it invites the one
false positive this step can produce: prose that happens to parse as a statement. The
complete-parse discriminator rejects most such text, and "most" is the wrong guarantee
when narrowing removes the class outright.

The default SHALL remain the whole workspace so that a workspace with no configuration
still bridges element text. Declaring roots SHALL be a narrowing choice, never the switch
that turns the step on.

The system MAY accept a dialect name scoped to those roots, with the same meaning it has
for `.sql` files: unset means the permissive cross-dialect parse, which is the normal and
usually better case. Naming a dialect SHALL NOT be presented as a performance control —
measured over a fixed statement set the permissive parse scored 13/16 against 10/16 for
oracle and postgres and 11/16 for mysql, so a named dialect is *stricter*, not better
informed. It is the same escape hatch it is elsewhere, for cases like T-SQL bracket
quoting.

#### Scenario: An unconfigured workspace still bridges

- **WHEN** no roots are configured
- **THEN** element text throughout the workspace is considered
- **AND** the step is not disabled by the absence of configuration

#### Scenario: Declared roots narrow what is tried

- **WHEN** a workspace declares a root holding its database XML
- **AND** documents outside it contain prose elements
- **THEN** only elements under that root are considered as SQL

#### Scenario: A root outside the indexed XML reports rather than silently yielding nothing

- **WHEN** a declared root holds no indexed XML, because the XML language's own roots or
  excludes never walked it
- **THEN** the step's report says so
- **AND** the result is distinguishable from a root that was searched and held no SQL

### Requirement: A workspace declares which attributes name tables

The system SHALL accept configuration naming the attributes that carry a table name,
and SHALL emit a table reference from every element carrying such an attribute. The
configuration MAY additionally bind an element name to an edge kind, so a reference
becomes `DefinesTable` or `AltersTable` rather than the `AccessesTable` an unbound
element yields.

With no such configuration the bridge SHALL still work from element text alone. A
workspace that supplies one attribute name SHALL reach tables that no element text
mentions.

The system SHALL NOT ship, infer, or special-case any framework's element names,
attribute names, or namespaces. Which vocabulary means "declares a table" is a property
of the tool a workspace chose, and building that list into kenn would make its
correctness depend on which tools its authors knew about.

#### Scenario: One configured attribute reaches attribute-declared tables

- **WHEN** a workspace configures an attribute name as carrying a table name
- **AND** its documents name tables only through that attribute
- **THEN** each such element emits a reference to the named table

#### Scenario: An element binding gives the reference its role

- **WHEN** the configuration binds an element name to the declaring edge kind
- **THEN** elements of that name emit `DefinesTable` references
- **AND** elements carrying the same attribute without a binding emit `AccessesTable` references

#### Scenario: No configuration still bridges element text

- **WHEN** no attribute is configured
- **THEN** references from element text are still emitted

#### Scenario: No vocabulary is built in

- **WHEN** the bridge's source is searched for framework element names, attribute
  names, or namespace URIs
- **THEN** none is found

### Requirement: Bridged edges are attributed to their element and graded

Every edge this step emits SHALL have the XML element that carried the reference as its
source, not the file, so a table's references identify the element that named it.

Every edge SHALL carry a `LinkGrade` decided by the same matching rule the SQL producer
uses: a qualified reference matches only its own schema, an unqualified one matches
every known table of that name, several matches are all kept and marked ambiguous, and
a name that is not statically knowable is dropped.

A reference matching no known table SHALL mint that table as external, exactly as a
reference in a `.sql` file does. A table exists in its database whether or not the
workspace declares it, and a workspace whose schema is owned elsewhere is the case this
bridge exists to serve.

A table SHALL be minted at most once however many bridged references name it. This step
is the first consumer that mints outside the pass owning the identity map, so it SHALL
extend that map as it mints rather than minting per reference — two elements naming the
same unknown table reach one node, as two statements in different `.sql` files already
do.

A bridged `DefinesTable` edge SHALL NOT be expected to change the table node's `external`
flag, which was written before this step ran. Ownership is read from the edge, not the
flag.

#### Scenario: An edge points at the element, not the file

- **WHEN** one document contains two elements that reference different tables
- **THEN** each edge's source is its own element
- **AND** neither is attributed to the document

#### Scenario: A bridged reference to an undeclared table links

- **WHEN** an element names a table nothing in the workspace declares
- **THEN** the table is minted as external
- **AND** the element's edge points at it

#### Scenario: Bridged and file references reach one table

- **WHEN** a `.sql` file declares a table
- **AND** an XML element references the same table by name
- **THEN** both point at the same table node

