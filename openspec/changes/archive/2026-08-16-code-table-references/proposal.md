## Why

kenn can index `.sql` and it can index XML, and neither connects to code. A table
node today is reachable from migrations and query files and nothing else, so the
questions people actually ask — *what code reads `users`*, *what tables does this
service touch*, *what breaks if I drop this column* — are unanswerable. The SQL work so
far builds a data-layer island.

The blocker was assumed to be literal values: SCIP carries symbols, ranges, and roles,
never the contents of a string, so "find the SQL in the code" looked like it needed
per-language sidecar work or a second parser. It doesn't. `defs.body_start_line` /
`body_end_line` already store every function's full extent — captured for `get source` —
so the source is addressable from the store, and `sql/parse.rs` was specified from the
start to accept SQL that did not come from a `.sql` file.

A spike over kenn's own index confirms it end to end. kenn embeds real SQL in Rust
string literals, so it is its own corpus:

```
bodies scanned            9,309
bodies with literals      4,103
bodies yielding tables      154
distinct tables              41
code→table edges            356

rs:kenn-collect::store::core::Store::end_session  ->  sessions
rs:kenn-collect::gc::Store::gc                    ->  commands, files, meta, sessions
```

The spike also found the one rule that decides whether this produces a graph or a mess.
Body extents **nest**: a module's span contains its functions', so slicing per symbol
gave the enclosing module every table its children touched. Attribution has to land on
the innermost enclosing symbol, or every ancestor inherits every descendant's edges and
the answer to "what touches `users`" becomes "most of the crate".

It found a real defect too, in shipped code rather than in the new path: a common table
expression read as a table, minting a `cnt` node from this repo's own recursive query.
Fixed separately — scanning real source surfaced in minutes what review had not.

## What Changes

Code symbols gain table references. A function whose body contains a SQL literal emits
the same `DefinesTable` / `AltersTable` / `AccessesTable` edges a `.sql` statement does,
to the same canonical table node, so `kenn list usages sql:users` answers with
migrations, query files, and application code in one homogeneous set.

Extraction reuses the existing extractor unchanged. What is new is getting literals out
of source — a per-language scan over a file's text, attributed to the innermost symbol
whose stored extent contains each literal.

Coverage is the honest limit rather than a hidden one. This finds SQL written as a
literal. Concatenated queries, builder APIs, and ORM-only codebases yield less or
nothing, and the reporting says which.

## Impact

- Affected specs: `code-table-graph`
- Affected code: a new post-code barrier step in `crates/kenn-indexer/`, reusing
  `sql/parse.rs` and the table registry from `index-sql`
- Depends on: `index-sql` (table identity, the extractor, the matching rule)
- Not affected: the `.sql` and XML producers, which keep emitting exactly what they do
  today; this adds a third source of references to the same nodes
