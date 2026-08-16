# SQL grades follow one-identity

## Why

`one-table-one-identity` changed how an unqualified table reference resolves, and
updated the two capabilities its deltas named — `code-table-graph` and
`xml-sql-bridge`. It did not update `sql-graph`, which specifies the same rule for
the `.sql` producer, and whose text the shipped code now contradicts:

> `Ambiguous` — an unqualified reference matches more than one known table. Every
> matching candidate SHALL be kept as its own graded edge; the system SHALL NOT
> choose one, and SHALL NOT discard them all.
>
> **Scenario:** … **THEN** an `AccessesTable` edge is emitted to each, graded
> `Ambiguous`

The code emits one edge, graded `Exact`. `LinkGrade::Ambiguous` still exists and
is still produced by the markdown, HTML and CSS producers — no table path
produces it.

The rule was implemented in `sql::registry::resolve`, which **all three**
producers share, so the behaviour changed for `.sql` files too. That was
deliberate — the same question must not get a different answer depending on which
file carried the reference — but only two of the three specs were updated.

## What changes

Spec only. No code changes: the behaviour shipped and is covered by
`an_unqualified_reference_adopts_the_one_schema_that_qualifies_it` and
`an_unqualified_reference_refuses_to_choose_between_two_schemas`.

`sql-graph`'s grading requirement is rewritten to state what the system does:
every table edge is `Exact`; an unqualified reference adopts the single schema
that qualifies its name and stands for itself when several do; and the count of
qualifying schemas is decided over the whole workspace rather than incrementally,
so identity does not depend on walk order.

## Why it was missed

The change's deltas were written from the two capabilities whose *files* it
touched, rather than from the set of capabilities whose *behaviour* it changed.
One shared function reached a third. Worth noting because the same slip produced
the `list_documents` omission earlier — both times, the delta list came from the
code rather than from the specs.
