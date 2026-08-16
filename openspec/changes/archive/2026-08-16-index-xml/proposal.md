## Why

kenn indexes markdown, stylesheets, HTML, and a text fallback, but not XML. So a
repo's XML — mapper files, changelogs, bean wiring, build manifests — is either
invisible or lands in the text fallback as opaque chunks, where its structure is gone
and only substring search survives.

XML is also the host format for a large share of the SQL that `index-sql` cannot see,
and for cross-references from configuration into code. Neither is reachable without an
XML structure first.

The temptation here is to write framework indexers — a MyBatis plugin, a Liquibase
plugin — the way prior art does. That is the wrong trade: it vendors a maintained list
of third-party element names into kenn, and it privileges the frameworks someone
happened to implement.

A spike (`tmp/xmlspike`) confirms it is unnecessary. Walking XML generically — element
path, `id`/`name` attribute, text content, byte range, namespace — recovers the same
structure those plugins extract:

```
mapper#0                          namespace="com.example.UserMapper"
mapper/select#id:findById         text: SELECT id, email FROM users …
databaseChangeLog/changeSet#id:0001
databaseChangeLog/changeSet/sql#0 text: ALTER TABLE users ADD COLUMN …
```

No framework is named anywhere in that walk.

## What Changes

- Add `Language::Xml`, claiming `.xml`, with a `[language.xml]` config section.
  Additional extensions (`.xsd`, `.xsl`, project files) are configurable, not
  built in: most are XML that other tooling owns, and claiming them by default would
  take files from producers that already handle them.
- Index every claimed file as **one barrier-free phase-1 sibling unit**, the shape
  `index-sql` and the text producer use: discover, parse, emit, all in one pass with
  no pending state for the post-code barrier.
- Emit a document node per file and **an element node per element**, related by the
  existing containment edge kinds. Element text and attributes ride on the element
  node; attributes are NOT separate nodes, which is what bounds the node count to the
  element count.
- Identify an element by its **ancestor-qualified path**, each segment discriminated
  by the element's `id`/`name` attribute when it has one and by its sibling ordinal
  otherwise. The spike proved a leaf-only ordinal collides — two `groupId` elements
  under different `dependency` parents both resolve to ordinal 0.
- Retain each element's text and make it **lexically** searchable, but keep it out of
  the embedding surface. XML content is configuration values, not prose — embedding it
  would cost vectors and embed time on every run while diluting the conceptual recall
  vectors exist for. This is also the span a later bridge reads SQL from, but this
  change does not read it.
- Parse with `roxmltree`: a read-only positioned DOM, one mandatory dependency
  (`memchr`), byte ranges via `Node::range()`, namespace resolution, and a positioned
  error rather than a panic on malformed input.

Out of scope, deliberately: reading SQL out of element text, resolving attribute
values against the symbol or table registries, and any edge leaving the XML document.
Those are the bridge, they need `index-sql` and the post-code barrier, and they would
drag framework-shaped configuration into a change that is otherwise pure structure.

## Capabilities

### Added Capabilities

- `xml-graph` — XML documents, elements, and their addressable structure in the code
  graph.
