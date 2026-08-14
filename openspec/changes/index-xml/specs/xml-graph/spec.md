## ADDED Requirements

### Requirement: XML is a first-class language with a configurable extension claim

The system SHALL index XML files under a `Language::Xml`, enabled and configured
through a `[language.xml]` section on the same footing as every other language. When
the section is disabled, no XML file SHALL be walked and no XML node SHALL be emitted.

The claim SHALL default to `.xml` alone. Further extensions SHALL be configurable and
SHALL NOT be built in: most other XML-shaped extensions belong to tooling that already
produces them, and claiming them by default would take files from an existing producer.

Indexing SHALL run as a single barrier-free phase-1 sibling unit: discovery, parsing,
and writing complete in one pass, leaving no pending state for the post-code barrier.

#### Scenario: Enabling the language indexes XML files

- **WHEN** `[language.xml]` is enabled and the workspace contains `.xml` files
- **THEN** each file is indexed as a document node with its elements
- **AND** the run report records the XML unit's files-seen count

#### Scenario: Other XML-shaped extensions are not claimed by default

- **WHEN** `[language.xml]` is enabled with no extra extensions configured
- **AND** the workspace contains project or schema files in an XML syntax
- **THEN** only `.xml` files are walked
- **AND** configuring an extra extension causes files with it to be walked

#### Scenario: One unusable file does not cost the others

- **WHEN** one claimed file is malformed and cannot be parsed
- **THEN** every other claimed file is still indexed
- **AND** the XML unit's report is degraded and names the file and the parse position
- **AND** the index run completes

### Requirement: Every element is a node, and every attribute is not

The system SHALL emit a document node per indexed file and an element node per element
in that file, related by the existing containment edge kinds so an element's ancestors
and descendants are walkable.

Attributes SHALL NOT be nodes. An element's attributes and its text SHALL ride on the
element node. This is what bounds the graph: node count tracks element count, and a
document with many attributes per element costs no more than one with none.

#### Scenario: Nesting is walkable in both directions

- **WHEN** a document nests an element three levels deep
- **THEN** each level is an element node
- **AND** the containment edges reach the innermost element from the document
- **AND** the innermost element's ancestors are reachable from it

#### Scenario: Attributes do not multiply nodes

- **WHEN** an element carries several attributes
- **THEN** exactly one node is emitted for that element
- **AND** its attributes are readable from that node

### Requirement: An element's id is its ancestor-qualified path

An element's public id SHALL be built from its file's workspace-relative path plus the
full chain of elements from the document root, where **every** segment carries its own
discriminator: the element's `id` or `name` attribute when it has one, and its ordinal
among same-named siblings otherwise.

Discriminating only the final segment is insufficient and SHALL NOT be done. Sibling
ordinals are counted within each element's own parent, so two elements under different
parents share a leaf ordinal — a build manifest with two `dependency` elements yields
two distinct `groupId` elements that both sit at ordinal zero, and a leaf-discriminated
id maps them to one node.

Preferring the `id`/`name` attribute keeps an id stable when the source offers
stability. The system SHALL NOT assume such an attribute is unique within a document;
the ancestor chain, not the attribute, is what guarantees uniqueness.

#### Scenario: Same-named leaves under different parents stay distinct

- **WHEN** a document contains two sibling `dependency` elements
- **AND** each contains one `groupId` element
- **THEN** the two `groupId` elements have different ids
- **AND** each id records which `dependency` it belongs to

#### Scenario: An identifying attribute is preferred to an ordinal

- **WHEN** an element carries an `id` attribute
- **THEN** its id segment is built from that attribute's value, not its position
- **AND** inserting a sibling before it does not change its id

#### Scenario: Elements without an identifying attribute fall back to position

- **WHEN** an element carries neither `id` nor `name`
- **THEN** its id segment is its ordinal among same-named siblings
- **AND** the id is still unique within the document

### Requirement: An element's markup and its content are stored as separate lossless surfaces

The system SHALL store an element's start tag as its signature surface and the element's
own text as its content surface, and SHALL NOT flatten either into the other or into a
search projection.

Both stored surfaces SHALL be lossless. The signature SHALL be well-formed XML markup —
the element's name and its attributes, rendered — so an attribute's name and value can
be recovered from it exactly. The content SHALL be the element's text verbatim.

This is how code is already stored: `symbol_docs.sig` holds a real signature
(`pub type AnalysisHook = Box<dyn FnOnce(…)>`), `symbol_docs.doc` holds the prose, and
the space-separated search text is *derived* from them at finalize rather than stored.
Storing a pre-flattened bag in the signature surface leaves no original to recover: a
space-joined `column name first name type varchar` cannot say whether the value was
`first` or `first name`, and gluing the tag onto the text yields `sql ALTER TABLE …`,
which no SQL parser accepts.

Losslessness here is not tidiness. A consumer reading these surfaces back — the
XML-carried SQL resolution among them — can then work from the store alone, which is
what allows the producer to stay barrier-free.

Text SHALL be attributed to the element that directly contains it, not to an ancestor,
so a document's stored units match its structure.

#### Scenario: An attribute's name and value survive the round trip

- **WHEN** an element carries an attribute whose value contains a space
- **THEN** the stored signature is markup from which that attribute's name and its full
  value are both recoverable
- **AND** neither is confused with the element's text

#### Scenario: Element text is stored verbatim

- **WHEN** an element's text is a SQL statement
- **THEN** the stored content is that statement exactly, with no tag name prefixed
- **AND** a parser reading the stored content accepts it

#### Scenario: Text belongs to its immediate element

- **WHEN** an outer element contains an inner element that holds the text
- **THEN** the text is attributed to the inner element
- **AND** the outer element does not claim it as its own

### Requirement: Element markup and text are both lexically searchable, and neither is embedded

The system SHALL derive an element's lexical search text from **both** stored surfaces —
its markup and its content — so an attribute value and a text value are equally findable,
and SHALL exclude XML from the embedding pass.

Deriving the lexical text from both surfaces is a projection, not a merge of the stored
columns: the same finalize step already derives code's lexical text from its signature by
identifier splitting. XML uses a different flattener — markup to words — because XML
content is structured *values* where the punctuation is the meaning, and splitting
`org.springframework` into words makes the substring someone would search for unfindable.

Routing content into the content surface would otherwise enrol XML in the embedding pass,
which selects on non-empty content with no language filter. XML element content is
overwhelmingly configuration values — identifiers, versions, type names, booleans — not
prose. Embedding it costs vector storage and embedding time on every index run and
dilutes recall for the conceptual queries vectors exist to serve. The exclusion SHALL
therefore be explicit; relying on the content surface being empty is no longer available
and SHALL NOT be assumed.

#### Scenario: A value in an attribute is findable

- **WHEN** an element carries an attribute holding a namespace or a version pin
- **THEN** that value is findable by substring search, spelled as written

#### Scenario: A value in element text is findable

- **WHEN** an element's text is a version pin
- **THEN** that value is findable by substring search, spelled as written
- **AND** it is attributed to that element

#### Scenario: Indexing XML produces no vectors

- **WHEN** a workspace containing only XML is indexed and the embedding pass runs
- **THEN** no vector is produced for any XML node
- **AND** the element's markup and text are both still findable lexically

### Requirement: Namespaces and byte ranges are recorded, not interpreted

Each element node SHALL record the resolved namespace of its name when it has one, and
the byte range it occupies in its file. Both are structural facts a later consumer
needs; this change SHALL NOT interpret either.

Recording the resolved namespace rather than the source prefix means the same element
is identified consistently however the document happens to bind its prefixes.

#### Scenario: A namespaced element records its resolved namespace

- **WHEN** a document binds a default namespace and nests elements under it
- **THEN** each element node records the resolved namespace
- **AND** the recorded value does not depend on the prefix the source chose

#### Scenario: Byte ranges locate an element in its file

- **WHEN** an element is indexed
- **THEN** its node records the byte range it occupies
- **AND** the range selects that element's source text

### Requirement: No framework, format, or schema is privileged

The system SHALL NOT recognize, name, or special-case any third-party XML vocabulary.
No element name, attribute name, or namespace URI belonging to a specific framework,
build tool, or schema SHALL appear in the implementation.

The only attribute names the indexer knows are the conventional identity carriers `id`
and `name`, which are properties of XML usage rather than of any vocabulary.

Where a specific vocabulary's meaning is genuinely needed, it SHALL be supplied as
configuration by the workspace that uses it, never shipped by kenn. A workspace whose
vocabulary kenn cannot interpret still gets its full structure, text, and ranges — only
the interpretation is absent.

#### Scenario: A framework's document is indexed without being recognized

- **WHEN** a document from any third-party XML vocabulary is indexed
- **THEN** its elements, text, attributes, ranges, and namespaces are all recorded
- **AND** no element or attribute of that vocabulary is treated specially

#### Scenario: The implementation names no vocabulary

- **WHEN** the XML indexer's source is searched for third-party element names,
  attribute names, or namespace URIs
- **THEN** none is found
