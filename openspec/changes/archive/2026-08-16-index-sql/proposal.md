## Why

kenn's `Language` enum has no SQL. A repo's schema — the tables its code depends on
— is invisible to the graph, so every schema question falls back to grepping a
migrations directory, which `CLAUDE.md` §10 exists to prevent. "Which queries touch
`users`?", "which statement created it", and "what has altered it since" are usages
questions with no usages answer.

The gap is structural, not cosmetic. A table is not like any node kind kenn indexes
today: its definition is a **fold over migration history**, not a definition site.

```
0001_init.sql      CREATE TABLE users
0007_email.sql     ALTER TABLE users ADD COLUMN email
0042_legacy.sql    ALTER TABLE users DROP COLUMN legacy
```

Prior art gets this wrong in a way worth learning from. CodeMap ships both halves —
a SQL DDL indexer and a MyBatis DML indexer — and they do not join, because the DDL
side scopes the table to the file that created it (`db/migrations/0001_init.sql/users#`)
while the DML side mints a bare name (`users`). Two nodes, no edge. Published research
surveys turn up no tool that unifies migration history with a code index under a
stable table identity.

This change indexes SQL and establishes that identity. It is deliberately scoped to
`.sql` files only — no code, no XML, no embedded SQL — so the identity model is
settled before anything is built on top of it.

## What Changes

- Add `Language::Sql`, claiming `.sql`, with a `[language.sql]` config section
  following the shape of the existing per-language sections.
- Index every `.sql` file as **one barrier-free phase-1 sibling unit** (the pattern
  the text producer establishes): discover, parse, resolve, and write in a single
  pass, with no pending state to resolve after the code join. The unit runs two
  internal passes — DDL first to build the table registry, then DML resolved against
  it — so statement order and file order do not matter.
- Mint **canonical table nodes** that are not file-scoped: `sql:<table>`, or
  `sql:<schema>.<table>` when and only when the DDL states a schema. Every DDL and
  DML statement in the workspace points at the same node.
- **Do not distinguish migrations from queries.** Both are `.sql`. A DDL statement
  defines; a DML statement accesses. Query files that application code loads at
  runtime are ordinary `.sql` files and are indexed identically — the distinction is
  a directory convention, and kenn does not infer conventions.
- Add `DefinesTable`, `AltersTable`, and `AccessesTable` edge kinds, and a statement
  node per top-level SQL statement so "which queries touch this table" resolves to
  specific statements rather than whole files. Definition and modification are
  separate kinds because a table is a fold over many statements, and "what created
  this" is a different question from "what has changed it".
- Parse each file **whole** first, splitting into statements only when no dialect
  parses it. Splitting first shears procedure and block bodies at their internal
  separators and loses the references inside them.
- Grade every table reference with the existing `LinkGrade`, including
  `Ambiguous` for an unqualified reference that matches more than one registered
  table. Unresolvable references produce **no edge and no node**, matching the CSS
  registry's "no dangling stubs" rule.
- Put SQL parsing in a **pure text-to-references module** with no file or store
  access, so the two later consumers (XML text spans, code string literals) share one
  implementation. `css/usage.rs` and `html/classes.rs` each define their own
  `ClassRegistry` today; that duplication is the outcome this constraint exists to
  prevent.

Out of scope, deliberately: column nodes, SQL embedded in any host language, XML, and
code-to-table edges. Those are follow-on changes that depend on this identity model
and would otherwise force it to be designed twice.

**Declarations are not a precondition for linking.** Measured on a real repository,
only 25 distinct tables are declared by `CREATE TABLE` in `.sql` files while 103 are
declared by a migration framework's XML attribute this change does not read. A design
that minted table nodes only from `CREATE` would therefore drop most references on that
workspace — and would be wrong to, because a table exists in its database whether or
not the repository declares it.

So any reference mints the table it names, and a `DefinesTable` edge marks a table
internal rather than admitting it. That mirrors what the system already does for code:
in one indexed workspace C# carries 605 internal symbols against 610 external ones, so
"referenced here, defined elsewhere" is the ordinary case, not a defect. A workspace
whose schema is owned by another service, an ORM, or a migration framework still gets
every statement that touches a table linked to one node.

## Capabilities

### Added Capabilities

- `sql-graph` — SQL files, statements, and canonical table identity in the code graph.
