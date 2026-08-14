## ADDED Requirements

### Requirement: SQL is a first-class language claiming `.sql`

The system SHALL index `.sql` files under a `Language::Sql`, enabled and configured
through a `[language.sql]` section on the same footing as every other language. When
the section is disabled, no SQL file SHALL be walked and no SQL node SHALL be emitted.

Indexing SHALL run as a single barrier-free phase-1 sibling unit: discovery, parsing,
reference resolution, and writing complete in one pass, leaving no pending state for
the post-code barrier. A failure in the SQL unit SHALL degrade that unit's `RunReport`
and SHALL NOT abort the run.

#### Scenario: Enabling the language indexes SQL files

- **WHEN** `[language.sql]` is enabled and the workspace contains `.sql` files
- **THEN** each `.sql` file is indexed as a file node with its statements
- **AND** the run report records the SQL unit's files-seen count

#### Scenario: Disabling the language emits nothing

- **WHEN** `[language.sql]` is disabled and the workspace contains `.sql` files
- **THEN** no `.sql` file is walked
- **AND** no SQL file, statement, or table node exists in the snapshot

#### Scenario: One unusable file does not cost the others

- **WHEN** one `.sql` file among several cannot be read or parsed at all
- **THEN** every other `.sql` file is still indexed with its statements and references
- **AND** the SQL unit's report is degraded and names the failure
- **AND** the index run completes

### Requirement: The dialect is configurable, recovery is exhaustive, and neither is inferred

The SQL dialect SHALL be selectable by name through configuration, defaulting to a
permissive cross-dialect parse when unset, so a workspace gets useful coverage with no
configuration. An unrecognized dialect name SHALL be a configuration error, never a
silent fallback.

When input fails to parse under the selected dialect, the system SHALL retry it
against the remaining dialects in a fixed order and accept the first that parses.
Recovery SHALL be deterministic for a given input. This yields better coverage than
any fixed dialect, because no single dialect accepts the most real-world statements —
not even for the database it is named after.

The system SHALL NOT infer or switch the configured dialect on its own, and SHALL NOT
narrow the retry set by syntax heuristics. Dialect names do not predict which parser
accepts which syntax — Oracle's outer-join operator parses only under the SQL Server
dialect — so a heuristic shortlist discards input an exhaustive retry recovers.

#### Scenario: Input the primary dialect rejects is recovered

- **WHEN** input uses quoting or syntax the selected dialect rejects
- **AND** some other dialect parses it
- **THEN** it is indexed with its table references
- **AND** the same dialect recovers it on every run

#### Scenario: An unset dialect still parses

- **WHEN** no dialect is configured
- **AND** the workspace contains statements from more than one dialect
- **THEN** statements that the permissive parse accepts are indexed
- **AND** naming a specific dialect is never required to index at all

#### Scenario: An unknown dialect name is rejected

- **WHEN** `[language.sql] dialect` names a dialect the parser does not provide
- **THEN** configuration loading fails with an error naming the unknown dialect
- **AND** the system does not fall back to the permissive default

### Requirement: A whole-file parse is attempted before a file is split into statements

The system SHALL first attempt to parse a `.sql` file as a whole, applying the dialect
recovery above. Only when no dialect parses the whole file SHALL it split the file on
top-level statement boundaries and parse each piece independently, again with dialect
recovery. A piece that still fails SHALL be dropped and counted.

The order matters and SHALL NOT be reversed. Splitting first loses references: a
procedure or anonymous block contains its own statement separators, so splitting shears
its body into fragments, and the leading fragment — carrying the block's opening
statement — fails to parse and takes its table references with it. A whole-file parse
keeps them.

Splitting SHALL still be attempted on whole-file failure, because a block whose body
carries no internal separators fails under every dialect while the statements following
it split out cleanly.

A split fragment that parses but references nothing SHALL NOT produce a statement node;
splitting a block body leaves trailing fragments that parse as empty statements.

#### Scenario: A parseable block keeps the references inside it

- **WHEN** a file contains a block that updates `accounts` and deletes from
  `stale_rows`, followed by a `CREATE TABLE ledger`
- **AND** some dialect parses the whole file
- **THEN** references to `accounts`, `stale_rows`, and `ledger` are all indexed
- **AND** the file is not split

#### Scenario: An unparseable block does not take the rest of the file with it

- **WHEN** a file opens with a block no dialect can parse
- **AND** a `CREATE TABLE` follows it
- **THEN** the whole-file parse fails and the file is split
- **AND** the table declared after the block is indexed
- **AND** the unparsed fragment is counted in the run report

#### Scenario: Empty fragments do not become nodes

- **WHEN** splitting a block body yields a fragment that parses with no table reference
- **THEN** no statement node is created for it

### Requirement: A table node is canonical, never file-scoped

A table SHALL be represented by exactly one node per identity across the whole
workspace, independent of which file declared it. The public id SHALL be
`sql:<table>` when the declaring statement names no schema, and
`sql:<schema>.<table>` when and only when the statement states one explicitly.

A table node SHALL be minted by **any** reference that names it, not only by a
declaration. A table exists in a database whether or not anything in the workspace
declares it: schemas are routinely owned by another service, managed by an ORM, or
older than the migration set that alters them. Requiring a declaration before a
reference may link would discard most of the graph's value on exactly those
workspaces.

A table that at least one statement declares SHALL be marked internal; a table only
ever referenced SHALL be marked external, in the same sense the system already applies
to symbols whose definitions lie outside the workspace. External tables are the normal
case, not a defect.

That mark records what **this pass** saw and SHALL NOT be treated as authoritative by a
consumer. The flag is written when the node is emitted, before any later producer has
contributed, so a table declared only outside `.sql` is written external and no later
pass corrects it. The authoritative signal for ownership is the presence of a
`DefinesTable` edge, which every producer contributes to and which no ordering can
stale.

The system SHALL NOT infer, default, or synthesize a schema for an unqualified
declaration. An unqualified `CREATE TABLE users` and a qualified
`CREATE TABLE analytics.users` are two distinct identities, because that is what the
source states.

Identity is minted from the declared form, but **reference resolution matches on the
table name with the schema as a discriminator**: a schema-qualified reference SHALL
match only the identity bearing that schema, while an unqualified reference SHALL
match every registered table of that name, whatever schema each carries. Engines
resolve unqualified names through a runtime search path the index cannot see, so an
unqualified reference genuinely may denote any of them.

#### Scenario: Statements in different files share one table node

- **WHEN** `0001_init.sql` contains `CREATE TABLE users`
- **AND** `0007_email.sql` contains `ALTER TABLE users ADD COLUMN email`
- **THEN** both statements link to the same `sql:users` node
- **AND** no file path appears in that node's id

#### Scenario: An explicit schema qualifies the identity

- **WHEN** a statement declares `CREATE TABLE analytics.users`
- **THEN** the table node id is `sql:analytics.users`
- **AND** it is a different node from `sql:users`

#### Scenario: A qualified reference does not match another schema

- **WHEN** both `sql:users` and `sql:analytics.users` are registered
- **AND** a statement references `analytics.users`
- **THEN** only `sql:analytics.users` is matched

#### Scenario: An unqualified reference reaches a schema-qualified table

- **WHEN** only `sql:analytics.users` is registered
- **AND** a statement references bare `users`
- **THEN** `sql:analytics.users` is matched

#### Scenario: A table nothing declares still links its references

- **WHEN** no statement anywhere declares `users`
- **AND** one statement selects from `users` and another drops it
- **THEN** a `users` table node exists and is marked external
- **AND** both statements link to that one node

### Requirement: Table identifiers are normalized before they become identity

A table name extracted from SQL SHALL have its dialect quoting removed before it is
used to mint a node or look one up. Backtick, bracket, and double-quote forms of the
same name SHALL resolve to a single table node.

Parsers return identifiers with their quoting intact. Left unnormalized, a quoted
reference both mints a duplicate identity and fails to match the registry — so a real
reference would be silently discarded by the no-stub rule rather than linked.

#### Scenario: Quoted and bare references reach one node

- **WHEN** one file declares `CREATE TABLE users`
- **AND** other statements reference `` `users` ``, `[dbo].[users]`, and `"users"`
- **THEN** every reference resolves against the same `users` identity
- **AND** no quoted variant appears as a separate table node

### Requirement: DDL defines, ALTER modifies, DML accesses, with no migration/query distinction

Every top-level SQL statement that references a table SHALL be emitted as a statement
node belonging to its file, carrying one edge per distinct table it touches:

- `DefinesTable` — the statement brings the table into being (`CREATE TABLE`).
- `AltersTable` — the statement changes the definition of a table that already exists
  (`ALTER TABLE`, `DROP TABLE`).
- `AccessesTable` — the statement reads or writes table data.

Separating definition from modification is what makes a table's history walkable: a
table is a fold over many statements, and "what created this" and "what changed it"
are different questions. One statement MAY emit edges of more than one kind — a
create-as-select both defines its target and accesses its sources — so the kind is a
property of the edge, never of the statement.

`DefinesTable` marks a table internal; it does not gate the table's existence. Every
edge kind above may mint the table it names, so a workspace that only ever alters or
queries a table still links every statement that touches it to one node. What
distinguishes a declared table from an undeclared one is the presence of a
`DefinesTable` edge, which is a fact the graph records rather than a precondition it
enforces.

The system records statements; it SHALL NOT evaluate them. A `DROP TABLE` contributes
an `AltersTable` edge and does NOT unregister the table, so statements referring to it
still resolve. The index is a fold over what the source says, not a simulation of
schema state at any point in time.

The system SHALL NOT classify a `.sql` file as a migration or as a query, and SHALL
NOT vary its treatment by directory, filename, or ordering convention. A file that
application code loads at runtime is indexed identically to one a migration tool
applies.

#### Scenario: A query file and a migration file are treated alike

- **WHEN** `db/migrations/0001_init.sql` contains `CREATE TABLE users`
- **AND** `queries/active_users.sql` contains `SELECT * FROM users`
- **THEN** the first statement emits `DefinesTable` to `sql:users`
- **AND** the second emits `AccessesTable` to the same node
- **AND** neither file is labelled by role

#### Scenario: Creation and modification are distinguishable

- **WHEN** one statement declares `CREATE TABLE users`
- **AND** a later statement in another file runs `ALTER TABLE users ADD COLUMN email`
- **THEN** the first emits `DefinesTable` and the second emits `AltersTable`
- **AND** the statement that created the table is identifiable without reading source

#### Scenario: One statement emitting two edge kinds

- **WHEN** a statement runs `CREATE TABLE report AS SELECT * FROM orders`
- **THEN** it emits `DefinesTable` to `sql:report`
- **AND** it emits `AccessesTable` to `sql:orders`
- **AND** it is one statement node, not two

#### Scenario: One statement accessing several tables

- **WHEN** a statement joins `users` and `orders`
- **THEN** it emits one `AccessesTable` edge per distinct table referenced
- **AND** table aliases introduced by the statement are not mistaken for tables

#### Scenario: A name a statement introduces for itself is not a table

- **WHEN** a statement defines a common table expression and selects from it
- **THEN** no table is emitted for the expression's name
- **AND** the tables the expression itself reads are still emitted
- **AND** a schema-qualified name identical to it is unaffected

#### Scenario: Altering a table nothing declares still links

- **WHEN** a statement runs `ALTER TABLE users ADD COLUMN email`
- **AND** no indexed statement declares `users`
- **THEN** a `users` table node exists, marked external
- **AND** the statement emits `AltersTable` to it
- **AND** a later query of `users` reaches the same node

#### Scenario: Dropping a table does not unregister it

- **WHEN** one statement declares `CREATE TABLE users`
- **AND** a later statement runs `DROP TABLE users`
- **AND** a third statement selects from `users`
- **THEN** the drop emits `AltersTable` to `sql:users`
- **AND** the select still emits `AccessesTable` to the same node

### Requirement: Table references are graded, and only unknowable ones are dropped

Every table edge SHALL carry a `LinkGrade` reflecting how its reference resolved:

- `Exact` — the reference matches exactly one known table under the matching rule
  above, or names a table nothing else in the workspace names. A `DefinesTable` edge is
  always `Exact`.
- `Ambiguous` — an unqualified reference matches more than one known table. Every
  matching candidate SHALL be kept as its own graded edge; the system SHALL NOT choose
  one, and SHALL NOT discard them all.
- Dropped — the reference's target is **not knowable statically**, for example a name
  supplied by runtime substitution. Only this case produces no edge and no node.

A reference SHALL NOT be dropped merely because nothing declares the table it names.
The dropped set is exactly the set of names the source does not determine — not the set
the workspace happens not to declare.

#### Scenario: An unqualified reference matching two tables keeps both

- **WHEN** both `sql:users` and `sql:analytics.users` are known
- **AND** a statement references bare `users`
- **THEN** an `AccessesTable` edge is emitted to each, graded `Ambiguous`
- **AND** neither candidate is silently preferred

#### Scenario: A runtime-substituted table name produces nothing

- **WHEN** a statement's table name is supplied by runtime substitution
- **THEN** no edge is emitted
- **AND** the substitution token does not become a table name

#### Scenario: An undeclared table is linked, not dropped

- **WHEN** every reference to a table is a query and none is a declaration
- **THEN** each query emits a graded edge to one shared table node
- **AND** that node is marked external rather than omitted

### Requirement: A statement's signature is its operation and the tables it names

Every statement node SHALL carry a signature naming the operation it performs and the
tables it names, spelled with the preposition SQL itself uses for that operation —
`ALTER TABLE users`, `SELECT FROM users, auth`, `UPDATE users`, `DELETE FROM users`,
`INSERT INTO users`.

The operation SHALL be taken from the parser's statement kind. It SHALL NOT be inferred
from the statement's rendered text, from the first tokens of the source, or from the
reference roles alone: roles collapse four operations into `Accesses`, and a token
heuristic needs a different prefix length per operation, trading an explicit mapping for
a silent one.

Coverage SHALL extend to every statement kind that can name a table, not to a chosen
few. The bound is narrower than it appears — a statement naming no table produces no
statement node at all, so the operations that need naming are only those that reach a
table. An unrecognized kind SHALL fall back to the reference role rather than to an
empty signature.

A statement that both defines and accesses SHALL be signed by what it defines; the
tables it reads remain reachable through their own edges, so the signature does not need
to restate them.

The signature SHALL NOT be capped or truncated. A join wide enough to be a problem does
not occur in practice, and a truncated signature would be the one form of this text that
cannot be searched for as written.

#### Scenario: Each operation is spelled with its own preposition

- **WHEN** a file contains a select, an update, a delete, and an alter
- **THEN** each statement's signature names its operation and its tables
- **AND** the preposition matches the one SQL uses for that operation

#### Scenario: Operations sharing a reference role stay distinguishable

- **WHEN** one statement queries a table and another updates the same table
- **THEN** their signatures differ by operation
- **AND** both carry the same reference role

#### Scenario: A multi-table query names every table it reads

- **WHEN** a statement joins several tables
- **THEN** its signature names all of them
- **AND** none is elided

#### Scenario: An unrecognized statement kind still signs

- **WHEN** a statement of a kind the mapping does not name references a table
- **THEN** its signature falls back to its reference role
- **AND** it is not left blank

### Requirement: Statement text is lexically searchable and is not embedded

A statement's full text SHALL be stored on its node as content and SHALL reach the
lexical search surface verbatim, and SQL SHALL be excluded from the embedding pass.

This is the same treatment XML receives, for the same reason. A statement is code, not
prose: it is identifiers, structure, and literals. Measured guidance in this repository
is that embedding value comes from comments rather than from signatures — signatures are
type and parameter noise while comments carry intent — and a statement is on the
signature side of that line. Embedding statements costs a vector per statement on every
index run and buys conceptual recall over text whose meaning is already carried by the
graph's table edges.

The lexical projection SHALL keep the text verbatim: no identifier splitting. A column
name, a constraint name, a default, or a type is spelled in the source the way someone
searching will spell it, and splitting `VARCHAR(255)` or `last_seen` into words makes the
substring they would type unfindable.

Verbatim statement text is what currently makes a **column** reachable at all. Columns
are not nodes: nothing below `SqlTable` and `SqlStatement` exists, so a column has no
name, no node, and no edge, and lexical search over statement text is its only access.
This SHALL be treated as a stopgap and SHALL NOT be treated as the permanent model — when
columns become nodes with their own edges, the graph answers those questions directly and
this surface may shrink to a true signature.

The exclusion SHALL be explicit. The embedding pass selects on non-empty content with no
language filter, so a producer that writes content is enrolled by default; relying on the
content surface being empty is not available to a producer whose content is its point.

#### Scenario: A column name is findable in the statement that introduced it

- **WHEN** a statement adds a column to a table
- **THEN** the column's name is findable by substring search
- **AND** it is attributed to that statement

#### Scenario: Indexing SQL produces no vectors

- **WHEN** a workspace containing only SQL is indexed and the embedding pass runs
- **THEN** no vector is produced for any SQL node
- **AND** the statement text is still findable lexically

#### Scenario: Statement text is not identifier-split

- **WHEN** a statement declares a column whose type carries punctuation
- **THEN** that type is findable spelled as written

### Requirement: SQL parsing is a pure module reusable by later producers

Reference extraction SHALL live in a module that maps SQL text to statement and table
references without reading files, opening the store, or knowing where the text came
from. The `.sql` producer SHALL be a consumer of that module, not its owner.

Later producers will hand this module SQL that did not come from a `.sql` file. The
system SHALL NOT grow a second table-reference extractor for any of them.

#### Scenario: The same text yields the same references regardless of origin

- **GIVEN** identical SQL text
- **WHEN** it is parsed as a `.sql` file's contents
- **AND** the same text is passed to the module directly
- **THEN** the extracted statement and table references are identical

#### Scenario: One extractor serves every origin

- **WHEN** the workspace is searched for SQL table-reference extractors
- **THEN** exactly one is defined

### Requirement: Table registry lookup has exactly one trait and one matching rule

Resolving a table name to registered table identities SHALL be expressed as a single
trait, and the rule deciding what a name matches SHALL be defined once against that
trait rather than by each consumer.

A consumer MAY supply its own source of identities — the `.sql` pass resolves against the
set it collected in memory, a later consumer against the same set read back from the
store — so implementations of the trait may be more than one. What SHALL NOT be more
than one is the matching rule: qualified-matches-its-own-schema, unqualified-matches-all,
several-kept-ambiguous, unknown-mints.

This repository has the opposite precedent: the same class-registry lookup is declared
twice, once for stylesheets and once for HTML, with two implementations of one job and
no shared rule between them. A second consumer of the table registry SHALL reuse the
trait and the rule rather than declare its own.

#### Scenario: One registry trait serves every consumer

- **WHEN** the workspace is searched for table-registry traits
- **THEN** exactly one is defined

#### Scenario: Every consumer resolves by the same rule

- **WHEN** two consumers backed by different identity sources resolve the same name
  against equivalent sets
- **THEN** they return the same candidates with the same grades
- **AND** neither defines its own matching logic
