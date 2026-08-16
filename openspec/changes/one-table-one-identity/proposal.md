# One table, one identity

## Why

Adding a single reference to a workspace changed an unrelated table's identity
and dropped its declaration. Found by `ddl-survives-a-partial-parse`'s §4.2
corpus run, which is the only reason it was noticed at all.

The chain, measured end to end on a real repository:

1. A Liquibase changeset carries two statements in one `<sql>` body:

   ```sql
   ALTER TABLE users.dealer_users RENAME TO dealer_assignments;
   ALTER TABLE users.dealer_assignments SET SCHEMA public;   -- unreadable
   ```

   `SET SCHEMA` parses in no supported dialect, so the block was previously
   discarded whole. With DDL now surviving a partial parse, the first `ALTER`
   contributes one reference — the **only** new reference on the whole corpus
   (1014 → 1015, verified against a controlled baseline).

2. That reference is **schema-qualified**, so it creates the identity
   `users.dealer_users`.

3. A `createTable` elsewhere declares the same table via an attribute that
   carries no schema, so it produces the **bare** name `dealer_users`.

4. `sql::registry::resolve` never unifies the two. A qualified reference mints
   `schema.name` without consulting the registry at all; a bare one resolves
   against `identities_named(name)`, which matches on name alone.

Result: the table's row moved from `sql:dealer_users` (internal, one `declares`)
to `sql:users.dealer_users` (external, one `modifies`). A reader who asked
"what declares this table?" got an answer before and gets none now.

## The two defects, separated

**D-A — a qualified and a bare reference to the same table do not unify.** A bare
name means *schema unstated*, not *schema empty*. Today it is treated as a
distinct identity, so one table becomes two nodes each holding half its
references — which is exactly the failure `normalize_table_name` already exists
to prevent for quoting and dotted spellings.

The naive fix is wrong and must not be taken: merging every same-named identity
would collapse `sales.orders` and `archive.orders`, which the atlas deliberately
keeps apart ("a name is a QUERY, not an identifier: two schemas can each hold an
`events`"). The rule has to be asymmetric — an *unqualified* identity is
under-specified and can be absorbed by a qualified one of the same name; two
*qualified* identities never merge.

**D-B — the mint guard keys on the bare name while the reference keys on the
whole identity, so an edge is silently dropped.** This is what actually loses the
declaration, and it is worse than the identity split that triggers it.

Both barrier steps do this (`code_sql/resolve.rs`, `xml_sql/resolve.rs`):

```rust
let candidates = resolve_name(known, r.schema.as_deref(), &r.name);  // key = (schema, name)
for c in candidates {
    if known.identities_named(&c.key.name).is_empty()          // guard: NAME only
        && seen_minted.identities_named(&c.key.name).is_empty() // guard: NAME only
    { minted.push(c.key.clone()); }
    out.refs.push(… table: c.key …);                            // ref: the FULL key
}
```

Once `users.dealer_users` is minted, the guard considers the name `dealer_users`
handled. The `createTable`'s bare reference then resolves to the *different* key
`dealer_users`, is not minted, and reaches `emit_table_edges` — which drops it:

```rust
let Some(target_id) = ids.get(table).copied() else { continue };
```

`emit.rs` justifies that skip as "a missing edge is a smaller wrong than a failed
run". That is defensible for an edge whose target genuinely does not exist, but
here it silently absorbs a declaration the workspace plainly makes. The table
ends with 1 reference where the corpus has 4.

**An earlier draft of this proposal blamed the wrong thing** — it suspected
`internal` was stamped at mint time and never revised, since `sql/emit.rs` writes
`external: true` for every minted table. That is **void**: `atlas/tables.rs`
computes `internal = sites.iter().any(|s| s.kind == RefKind::Declares)` from the
references a table ended up with, and even has a test called
`a_declaration_anywhere_marks_the_table_internal`. The node flag is not what the
axis reads. The declaration is missing because its *edge* never got written.

## What changes

- A qualified reference absorbs an existing unqualified identity of the same
  name, rather than minting a sibling. Two qualified identities are left alone.
- The mint guard and the emitted reference agree on what an identity is. Whatever
  key a reference carries is the key that gets minted, so no reference can point
  at a node that was never written.
- A reference whose target is absent is **counted**, not silently skipped. The
  skip is a reasonable last resort; being unable to see it happen is not, and it
  is why this cost a full investigation to find.

## Why it is worth doing rather than papering over

Any change that adds declarations can trigger this, so it is not specific to
partial-parse recovery — it is a standing hazard for every future indexing
change, and it makes the internal/external census unreliable as a before/after
measure. It has already cost one investigation this session.

It also silently halves a table's reference set whenever a codebase mixes
qualified and unqualified spellings of the same table, which is the common case
in migration tooling: `<createTable schemaName="…" tableName="…">` beside a
hand-written `ALTER TABLE schema.name`.

## Not in scope

- Reverting DDL recovery. It is a large net gain (kenn's own schema went from
  invisible to 29 of 48 tables declared) and it did not cause this — it only
  made a pre-existing hazard observable.
- Teaching the parser `SET SCHEMA`. Would remove this instance and leave the
  hazard.
