## Why

`index-sql` reads `.sql` files and `index-xml` reads XML structure, and neither can see
the other. On a real repository measured during design that is where most of the schema
lives: 25 distinct tables are declared by `CREATE TABLE` in `.sql` files, while 103 are
named by an XML attribute — and 1008 XML elements carry SQL text in their bodies. Two
producers, both correct, and the join between them missing.

The join cannot live in either producer. A `<select>` body is SQL that the XML producer
has no business parsing, and it sits in a file the SQL producer never opens. It belongs
in a phase that runs after both.

kenn already has that phase. The pipeline joins its parallel producers at a barrier and
then runs three cross-producer resolutions — markdown links to code, stylesheet classes
to code, HTML to code and stylesheets — each reading the building store once every
producer has written to it. This change adds a fourth resolution of the same shape,
and does not invent a mechanism.

What it must not do is teach kenn a framework. The vocabulary that says "this attribute
names a table" belongs to whichever migration tool a workspace chose, and shipping a
list of them would make kenn's correctness a function of which tools its authors had
heard of. The workspace declares its own vocabulary instead.

## What Changes

- Add a **post-code barrier resolution** for cross-producer SQL joins, alongside the
  existing markdown, stylesheet, and HTML barrier steps. It reads XML element nodes and
  the table registry from the building store after every producer has joined, so
  neither `index-sql` nor `index-xml` needs deferred state and both stay barrier-free.
- **Parse SQL out of XML element text.** Any element whose text parses as SQL
  contributes the same table references a `.sql` statement would, through the same
  shared extractor, with the same dialect recovery and the same grading. An element
  whose text is not SQL contributes nothing and is not an error — most element text is
  not SQL.
- **Let configured attributes name tables.** A workspace declares which attribute names
  a table, and optionally which element gives that reference the role of declaring,
  modifying, or accessing one. With no configuration the axis still works from element
  text alone; with one line it reaches the attribute-declared schema that text cannot.
- **Attribute every bridged edge to the element it came from**, so a table's references
  point at the `<select>` or `<createTable>` that named it rather than at the file.
- Grade bridged edges with the existing `LinkGrade`, and follow the asymmetry the two
  registries genuinely have: a table reference that matches nothing **mints an external
  table**, because a table exists in its database whether or not the workspace declares
  it, while nothing is minted for a name that should have been in the workspace and
  is not.

Out of scope: SQL embedded in source-code string literals, joining XML attribute values
to code symbols, and migrating the existing markdown, stylesheet, and HTML resolutions
onto a shared bridge abstraction. All three reuse this phase, none of them changes it,
and folding any of them in would mix a new capability with a refactor of working code.

## Capabilities

### Added Capabilities

- `xml-sql-bridge` — cross-producer resolution of SQL and table references carried by
  XML.
