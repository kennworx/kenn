# DDL survives a partial parse

## Why

kenn cannot see its own schema. `kenn tables` reports `symbols`, `defs`, `edges`,
and `packages` as **referenced-only** — tables some code touches but nothing in
the workspace declares — while `crates/kenn-store/src/db/sqlite/schema.rs` plainly
creates all of them.

The cause is one line in `code_sql::resolve::refs_of_literal`:

```rust
let ex = extract(t, None);
if ex.unparsed > 0 {
    return Vec::new();      // the whole literal, discarded
}
```

Measured against the real `GRAPH_DDL` constant:

| | |
|---|---|
| literal size | 4100 bytes |
| statements that parsed | **26** |
| statements that failed | **1** |
| refs carried by the 26 | 14 `Defines`, 12 `Accesses` |
| verbs of the 26 | 14 `CREATE TABLE`, 12 `CREATE INDEX`, **zero DML** |
| kept today | **0** |

One statement the parser cannot read costs twenty-six it can. The statement in
question is `CREATE VIRTUAL TABLE name_words USING fts5(words,
tokenize='unicode61')`, and `sqlparser` rejects it in **all 14 dialects** — the
named argument `tokenize=` is the parse error, and `USING vec0(embedding
float[768])` fails the same way. Confirmed with a spike, not inferred:

```
virtual fts5   generic=ERR   sqlite=ERR   Expected: ), found: = at Column 59
virtual vec0   generic=ERR   sqlite=ERR   Expected: ), found: float
```

So this is not a kenn-specific accident. **Any SQLite codebase using FTS5 or
sqlite-vec loses its entire schema constant**, and any codebase whose schema blob
contains one vendor extension loses everything around it.

## What the guard is actually for

The rule is not wrong. It exists because a *runtime-assembled query fragment*
splits into pieces that parse as something they are not — `") SELECT id FROM
temp_results"` yields `temp_results` as a table when it is a CTE defined in
another literal. That is a real false positive and the guard prevents it.

The defect is that the guard cannot distinguish two very different literals:

```
a runtime-assembled fragment          a complete multi-statement DDL blob
─────────────────────────────         ───────────────────────────────────
one statement, torn                   many statements, each whole
leftovers are dangling clauses        leftovers are whole statements the
                                        parser does not know
a name may be an alias or CTE         a name is a schema object by position
dropping it is correct                dropping it costs everything
```

## What changes

On a partial parse, keep the references made by statements whose verb is **DDL**;
keep dropping everything else.

The measurement is what picks this rule over the alternatives. A DDL statement
names a schema object *by grammatical position* — there is no such thing as a CTE
in the target slot of `CREATE TABLE`, `CREATE INDEX … ON`, or `ALTER TABLE`. A
query does not have that property, which is exactly why the guard was written.

Two rules that look reasonable and are wrong, both ruled out by the numbers above:

- **Filter by ref role** (keep `Defines`/`Alters`, drop `Accesses`) would discard
  the 12 `CREATE INDEX` references, which are as safe as the 14 `CREATE TABLE`
  ones — an index's target is a real table. Role is a property of what the
  statement does to the table; the trustworthiness lives in the *verb*.
- **Relax the guard entirely** would readmit the fragment false positive the
  guard was written for, which no measurement here justifies.

## Scope

Three call sites share this policy. Two apply it and one does not:

| site | today | after |
|---|---|---|
| `code_sql::resolve::refs_of_literal` | drops the literal | keeps DDL statements |
| `xml_sql::resolve::refs_from_text` | drops the element text | keeps DDL statements |
| `sql::ingest` (`.sql` files) | already keeps them, counts `unparsed` | unchanged |

The `.sql` producer has always kept partial results — it only *counts* `unparsed`.
So this change does not invent a policy; it brings the two literal-bearing
producers to the one the file producer already uses, narrowed to the statements
whose names cannot lie.

Both literal sites move together. Fixing one and leaving the other would put the
same schema blob's fate down to whether it was pasted into a `.cs` file or a
Liquibase `<sql>` block.

## Not in scope

- **The ORM ceiling.** 388 of 4442 C# files in the corpus measured reach tables
  through Entity Framework or Dapper, where no SQL text exists to parse. Raising
  that needs an entity-to-table producer, not a better parser.
- **Teaching `sqlparser` about `fts5` / `vec0`.** Upstream dialect work would
  reduce how often this fires but not change the policy question, and the policy
  is wrong today independent of any one extension.
- **Temp tables.** `CREATE TEMPORARY TABLE tmp_x` already registers as a declared
  table on a clean parse. This change does not make that better or worse, and it
  is a separate question about what counts as a schema object.
