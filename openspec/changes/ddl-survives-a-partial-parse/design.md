# Design

## D1 — The discriminator is the verb, not the role and not the count

Three candidate discriminators were considered. The measurement on `GRAPH_DDL`
(26 parsed statements, 1 failed, verbs `14 × CREATE TABLE` + `12 × CREATE INDEX`,
roles `14 Defines` + `12 Accesses`) eliminates two of them:

| discriminator | recovers GRAPH_DDL | readmits the fragment false positive |
|---|---|---|
| statement count ≥ 2 | yes | **yes** — a torn query can split into 2+ pieces |
| ref role ∈ {Defines, Alters} | **partially** — loses the 12 `CREATE INDEX` | no |
| **statement verb is DDL** | **fully — all 26** | no |

Role fails because role describes *what the statement does to the table*, while
the property that matters is *whether the name can be something other than a
table*. `CREATE INDEX ix ON symbols(id)` has role `Accesses` and a target that is
necessarily a real table. Those two facts are independent, and only the verb
carries the one we need.

Statement count fails because it is a proxy for "this looks like a script", and a
concatenated query torn across three literals also produces several pieces.

## D2 — Why a DDL verb is trustworthy under a partial parse

A CTE or alias is introduced by a query and is visible only inside it. The
grammar has no production that puts such a name in the target position of
`CREATE TABLE`, `CREATE INDEX … ON`, `ALTER TABLE`, or `DROP TABLE`. So a name
read out of a *successfully parsed* DDL statement is a schema object regardless
of what the bytes around that statement were.

This is a statement about the grammar, not a heuristic about codebases, which is
why it holds under a partial parse when nothing else does.

Note the guarantee is about the surviving statements, not about the blob. We are
not claiming the blob was well-formed; we are claiming that each statement the
parser *did* accept, and that is DDL, names real tables.

## D3 — Where the filter goes

In `sql::parse`, not at the two call sites.

`ParsedStatement` already carries `verb: Option<String>` (added by `index-sql`
§"SQL statement signature"). The predicate belongs next to `verb_of`, which is
the function that decides the vocabulary — putting it at the call sites would
duplicate the verb list twice and let the two producers drift.

The shape is one function plus one filter, with the existing `unparsed` field
untouched so `.sql` reporting keeps working:

```rust
/// Whether a statement's table names are trustworthy when the blob around it
/// did not fully parse. See design D2.
pub fn names_are_positional(verb: Option<&str>) -> bool
```

Call sites then read:

```rust
let ex = extract(t, dialect);
let trusted = if ex.unparsed == 0 { All } else { DdlOnly };
```

## D4 — What the two call sites keep in common

They keep the pre-filters (`len() < 12`, `looks_like_sql`) and the "a non-parse is
ordinary text, never a reported failure" contract. Neither is affected: this
change is only about what happens *after* `extract` returns with `unparsed > 0`.

`xml_sql::refs_from_text` returns a `bool` meaning "this text was SQL at all",
used for reporting. Under the new rule a text whose DDL survived is SQL, and a
text whose statements were all dropped is not — the boolean keeps meaning what
its callers already assume.

## D5 — Why not simply teach the parser `fts5` / `vec0`

Considered and rejected as the *fix*, though worth doing separately. It would
make this particular blob parse whole, and the next vendor extension would
reintroduce the same cliff. The defect is that one unreadable statement is
allowed to cost every readable one; that is true whatever the unreadable
statement happens to be.

## D6 — Risk, stated plainly

This change admits references that today are refused. The new false-positive
surface is exactly: **a runtime-assembled fragment that (a) fails to parse whole,
(b) splits into a piece that parses as valid DDL, and (c) names something that is
not a table.** For that to happen a codebase would have to assemble `CREATE
TABLE`/`ALTER TABLE` text at runtime *and* have the assembled-away part change
which object is named.

That is not hypothetical — dynamic DDL exists (multi-tenant table-per-tenant
schemes build `CREATE TABLE tenant_{id}_orders`). What such a case produces is a
table node for the *template* name, which is a wrong name rather than a
misclassified alias. The mitigation is not to keep dropping every schema constant;
it is that these names are already reported with a grade, and a reader can see
the reference site.

Verification (§4) includes a corpus run to size this, because "no measured
regression" is the only honest way to claim the risk is small.
