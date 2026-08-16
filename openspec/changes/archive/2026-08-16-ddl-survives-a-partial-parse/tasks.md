## 1. The predicate

- [x] 1.1 Add the predicate to `sql::parse`, next to `verb_of` so the verb
  vocabulary has one home. True for the DDL verbs whose target slot cannot hold a
  CTE or alias; false for everything else, including an unmapped kind.

  Landed as `Verb::names_positional`, an exhaustive `match` on a new `Verb` enum
  — **not** as the free function over rendered strings this task first described.
  See §7 for why that was replaced. → verify: the compiler, not a test; a new
  `Verb` variant does not compile until it is classified.
- [x] 1.2 `None` is false, deliberately: a statement whose verb we could not name
  is not one whose names we can trust. → verify: asserted directly.

## 2. The two literal-bearing call sites

- [x] 2.1 `code_sql::resolve::refs_of_literal` — on `unparsed > 0`, keep the refs
  of statements satisfying §1.1 instead of returning empty. → verify: the
  `GRAPH_DDL` regression test in §3.1.
- [x] 2.2 `xml_sql::resolve::refs_from_text` — same change, same predicate. Its
  `bool` return keeps meaning "this text was SQL at all". → verify: an element
  whose `<sql>` body mixes a `CREATE TABLE` with an unparseable statement yields
  the table.
- [x] 2.3 Rewrite both doc comments. Both currently say a partial parse is
  where an alias reads as a table and stop there, which will be half the story.
  `code_sql`'s also claims the whole-then-split tiering "is deliberately not
  used", which was never true — it calls `extract`, which tiers, and then discards
  the result. → verify: neither comment states something the code does not do.
- [x] 2.4 Leave `sql::ingest` alone. It already keeps partial results and counts
  `unparsed`; this change moves the other two toward it, not the reverse. →
  verify: `ingest.rs` is untouched by the diff.

## 3. Regression coverage

- [x] 3.1 A test over a schema-constant shape: several `CREATE TABLE` /
  `CREATE INDEX` statements plus one `CREATE VIRTUAL TABLE … USING fts5(words,
  tokenize='unicode61')`. Assert every real table is found and the virtual one is
  not. → verify: mutation-checked per CLAUDE.md §9 — restore the `return
  Vec::new()` and this test goes red **for the stated reason** (zero refs, not a
  wrong count).
- [x] 3.2 A test that the fragment guard still holds: a torn query fragment whose
  split piece parses as a `SELECT` naming a CTE contributes nothing. → verify:
  mutation-checked — drop the verb predicate (trust everything on a partial
  parse) and this goes red. **Both mutations are required**: 3.1 alone would pass
  with the guard removed entirely, and 3.2 alone would pass with it left as-is.
  Neither test is a guard without the other.
- [x] 3.3 Use the *real* `fts5` spelling, including `tokenize='unicode61'`. The
  named argument is the parse error; a simplified `USING fts5(words)` may parse
  and the fixture would then never reach the branch it names — the §9 "suspect the
  fixture" failure mode this repo has hit repeatedly. → verify: assert
  `extract(…).unparsed > 0` inside the test, so the fixture proves it reaches the
  partial-parse path rather than assuming it.

## 4. Measure the change on real corpora

- [x] 4.1 Self-index: `kenn tables` must report `symbols`, `defs`, `edges`,
  `packages`, `aggregate_*`, and `analysis_*` as **declared in-repo**. Today 14 of
  49 tables are internal. → verify: record the before/after internal count here.
- [x] 4.2 A multi-language corpus (C# + Liquibase XML + SQL): record tables,
  references, and per-language site counts before and after.

  **This gate failed on the first attempt and is what found `one-table-one-identity`.**
  Against a controlled baseline, one table moved from `sql:dealer_users`
  (internal, `declares`) to `sql:users.dealer_users` (external, `modifies`) — a
  visible lost declaration. The cause was not in this change, which can only add
  references, but in a mint guard keyed on a table's bare name while references
  carried the whole key; this change's one new reference merely exposed it.

  With that fixed, the gate passes: no table name loses references, `dealer_users`
  carries **both** its declaration and its `ALTER` (2 references, where the
  pre-change baseline had 1), and the run reports zero dropped references.
- [x] 4.3 Size the D6 risk rather than asserting it is small.

  Answerable exactly, because this change was measured alone before the identity
  fix landed. References admitted by the relaxation:

  | corpus | admitted | what they were |
  |---|---|---|
  | multi-language (1014 refs) | **1** | `ALTER TABLE users.dealer_users RENAME TO dealer_assignments`, in a `<sql>` body whose second statement (`SET SCHEMA`) no dialect reads |
  | self-index (281 refs) | **26** | every statement of `GRAPH_DDL` — 14 `CREATE TABLE` + 12 `CREATE INDEX` |

  **Zero names that are not real tables**, across both. The sample is small
  enough to have been read in full rather than sampled: 27 references, all of
  them DDL naming a schema object in its target slot, which is exactly what the
  verb rule predicts. The D6 hazard — dynamic DDL such as
  `CREATE TABLE tenant_{id}_orders` producing a template name — did not occur in
  either corpus, and remains a real possibility rather than an observed one.

## 5. Gates

- [x] 5.1 `just test`, then `cargo clippy --workspace --all-targets`, then
  `just crap-ci`, then `cargo fmt --all`, then clippy once more (CLAUDE.md §7).
- [x] 5.2 Watch the gate on `refs_of_literal` and `refs_from_text`: both gain a
  branch. If either crosses 30, add coverage rather than baselining (CLAUDE.md §6).

## 6. Close the loop

- [x] 6.1 Resolve the finding this change came from
  (`fnd_37c61ac0-be0c-4f40-832c-c6ada89c16cc`). Superseded by
  `fnd_b4baf13d-adcc-4df0-a5d8-7a41f88874a1`, not deleted: the original
  measurement — `sqlparser` rejecting `USING fts5(…, tokenize='unicode61')` in all
  14 dialects — is still the reason the rule is what it is, and the next person to
  touch `refs_of_literal` needs it.

## 7. What changed during implementation

**§1.1's first implementation was replaced.** The predicate began as a free
`names_are_positional(Option<&str>)` matching on rendered verb strings, with a
test that read this file back via `include_str!`, scraped `verb_of`'s arms for
`=> "…"`, and failed unless each verb was named in the predicate's own source.
It worked and was mutation-checked three times — but it parsed Rust with string
splitting, and a `rustfmt` line-wrap between `=>` and the literal would have
dropped a verb from the enumeration *silently*. A guard that can quietly stop
guarding is the failure mode §9 exists for, and holding the test to a lower
standard than the code would have been the wrong trade.

Replaced with an enum. `verb_of` now returns `Option<Verb>`; `Verb::as_str`
renders the same user-visible strings, and `Verb::names_positional` is an
exhaustive `match`. Verified the way the scraper could not be: adding a
`Verb::Vacuum` variant fails to compile in **both** matches —

```
error[E0004]: non-exhaustive patterns: `Verb::Vacuum` not covered  (as_str)
error[E0004]: non-exhaustive patterns: `Verb::Vacuum` not covered  (names_positional)
```

so a new statement kind cannot be added without deciding both how it renders and
whether its names survive a partial parse. The enumeration test is deleted; what
remains is behavioural (which verbs are positional, and that the `DROP` variants
keep distinct renderings).

## 8. Status — NOT ready to archive

§4.2 and §4.3 are open, and §4.2 found something real.

**The self-index is exactly what was wanted** (§4.1): internal tables 14 → 29 of
48, with `symbols`, `defs`, `edges`, `packages`, `aggregate_*`, and `analysis_*`
all now declared in-repo. Per-table, nothing lost references. `vec_knowledge`
stays external, correctly — its `vec0` virtual table still does not parse.

**The corpus run did not pass its own gate.** Against a controlled baseline (the
change stashed, rebuilt, reindexed today — not the stale 08-14 index), one table
moved from `sql:<name>` (internal, one `declares`) to `sql:<schema>.<name>`
(external, one `modifies`). Reproducible, and byte-identical across two runs, so
not nondeterminism.

The mechanism is NOT in this change's logic, which can only ever *add*
references. `sql::registry::resolve` treats a schema-qualified reference and a
bare one as different identities that never unify: a qualified ref always mints
`schema.name` without consulting the registry, while a bare ref resolves against
`identities_named(name)` and mints the bare form only when nothing is known.
Growing `known` — which any new declaration does — therefore flips bare refs onto
a qualified identity and re-attributes the row.

So this is a **pre-existing fragility this change exposes**, not one it
introduces. That does not make it shippable: a user would see a table lose its
declaration. Open questions before archiving:

- [x] 8.1 Should the two spellings unify? **Yes, asymmetrically** — split out as
  `one-table-one-identity`, where it is written up with the full chain measured.
  The naive answer is wrong: merging every same-named identity would collapse
  `sales.orders` and `archive.orders`, which the atlas deliberately keeps apart.
  A *bare* name means schema unstated and can be absorbed by a qualified one; two
  qualified names never merge.

  Traced to the end while writing that up. The single new reference this change
  admits on the corpus is `ALTER TABLE users.dealer_users RENAME TO
  dealer_assignments`, in a `<sql>` body whose second statement (`ALTER … SET
  SCHEMA public`) no dialect reads — so the block used to be dropped whole. That
  reference is schema-qualified, the `createTable` that declares the same table
  carries no schema, and the two never unify. One new reference, one table
  re-attributed.
- [x] 8.2 The census must be diffed per `pub_id`, not per name — recorded as
  `fnd_601a3fe6-3fe2-4f5b-a3c1-c9339022a481`, and promptly violated: the §4.2
  diff was keyed by name. Re-checked afterwards and it happened to be sound
  (no two identities share a name on that corpus), but that was luck, not method.
- [x] 8.3 `4.2`, `4.3` and `6.1` stayed open until `one-table-one-identity`
  landed. It has: the reference loss is fixed, the corpus gate passes, and this
  change is archivable.
