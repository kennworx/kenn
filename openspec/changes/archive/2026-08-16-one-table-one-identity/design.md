# Design

## D1 — Adoption is not a rule, because a bare name is not always answerable

The proposal assumed a bare reference and a qualified one of the same name are
"almost certainly the same table". Measured on the multi-language corpus, that is
true often and **not always**, and the exceptions are not rare:

| name | identities |
|---|---|
| `transfers` | `wallets.transfers` (96) · `transfers` (83) · `public.transfers` (48) |
| `referrals` | `referrals` (12) · `users.referrals` (10) · `public.referrals` (3) |
| `orders` | `orders` (70) · `bots.orders` (9) |

23 of 158 identities are names carrying more than one spelling, and several carry
**two different qualified** spellings. For those, "which schema does the bare
`transfers` belong to" has no answer in the reference itself — `wallets` and
`public` are different schemas holding a table of the same name, exactly the case
the atlas refuses to merge.

So the first implementation's rule — adopt the bare identity — was not merely
order-dependent (which is why it collapsed `sales.orders` into `archive.orders`).
It was answering a question the data does not answer.

## D2 — The order dependence is the one-pass shape, and that is fixable

Both barrier steps decide an identity the moment they first see a name. The
`.sql` producer does not: it collects the full identity set from *every*
reference first, then resolves each reference against it, precisely so "a query
in an early-sorted file can reach a table declared in a later one".

The barrier steps should do the same. Two passes:

1. Collect every raw `(schema, name)` a reference makes.
2. Decide the identity set: every **qualified** spelling is an identity.
3. Resolve each reference against `known ∪ that set`.

This removes order dependence by construction rather than by patching the
symptom, and it needs no new policy — `resolve` already handles a bare name
matching several identities by returning **all** of them graded `Ambiguous`
("keep them all rather than choose or discard").

## D3 — The open question, and why it is not mine to settle

Under D2, what becomes of the 83 bare `transfers` references?

**Option A — fan out.** They match `wallets.transfers` and `public.transfers`,
so each emits an edge to both, graded `Ambiguous`. Consistent with the existing
rule for unqualified names. Cost: 83 references become 166 edges, and a reader
counting references to `wallets.transfers` sees references that may belong to
`public.transfers`.

**Option B — keep a bare identity.** `sql:transfers` survives as its own node,
meaning "referenced without a schema, and we will not guess". Cost: the atlas
shows three rows for what may be two tables, and a reader asking "who touches
`transfers`" must union them.

**Option C — fan out only when unambiguous.** A bare reference adopts the single
qualified spelling when exactly one exists (the `orders`/`bots.orders` case, and
`dealer_users`), and keeps a bare identity when two or more do. Splits the
difference: fixes the common case, refuses to guess the genuinely ambiguous one.

C is the recommendation — it is the only one that never invents a fact — but the
choice is about what the atlas should *say*, not about correctness, so it wants
a decision rather than a default.

## D4 — What is already fixed and does not depend on this

The reference *loss* is fixed and shipped: every reference now reaches a node
that exists, dropped references are counted and warned, and no name lost
references across the corpus. D1–D3 are about how many rows a table occupies,
not about whether its references survive. That distinction is why the two halves
were separated rather than held together.

## D5 — Decision: C, and what it cost

**C was chosen.** A bare reference adopts the one schema that qualifies its name,
and stands for itself when several do. Implemented in `sql::registry::resolve`
for *every* producer, not only the barrier steps — the same question must not get
a different answer depending on which file carried the reference.

That replaces the previous rule (option A, fan out to every candidate graded
`Ambiguous`), so its test changed with it. A's flaw is visible in the measurement:

| | fan-out (A) | C |
|---|---|---|
| identities | 158 | **147** |
| names split across spellings | 23 | **10** |
| references | 1482 | **1180** |

The reference count *falls by 302*, and that is the point rather than a
regression. `transfers` shows it exactly: under A its three identities held
96 + 83 + 48 = 227 references; under C they hold 83 + 15 + 3 = 101. The missing
126 were one reference counted against two schemas — a reader asking how much
code touches `wallets.transfers` was told 96 when 15 of them said so.

C also merges what A left split: `orders` was `orders(70)` + `bots.orders(9)`
and is now `bots.orders(72)` + `orders(1)`, because exactly one schema qualifies
that name so the bare references mean it.

`transfers` and `referrals` keep three rows each, and that is C working, not C
failing: two schemas qualify those names, so the bare references genuinely do not
say which is meant.

## D6 — Two passes are load-bearing for C, not a tidy-up

C asks "how many schemas qualify this name", which is only answerable over the
whole workspace. Deciding it incrementally makes the answer depend on walk order:
a bare `transfers` read after `wallets.transfers` but before `public.transfers`
adopts `wallets`, while the same reference read one file later stands for itself.

So both barrier steps now collect every raw reference first and resolve second,
which is the shape the `.sql` producer already had for the same reason. The
one-pass version is mutation-checked: reverting it fails
`a_name_with_two_schemas_resolves_the_same_in_any_order` on the `bare middle`
ordering with 2 identities instead of 3.
