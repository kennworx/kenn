## 1. Settle what is actually broken

- [x] 1.1 Locate the emitter that stamps a minted table's `external` flag and
  determine whether it is ever revised. **The original D-B is void.**
  `sql/emit.rs:71` does write `external: true` for every minted table, but the
  axis never reads that flag: `atlas/tables.rs:104` computes
  `internal = sites.iter().any(|s| s.kind == RefKind::Declares)` from the
  references the table ended up with, and `a_declaration_anywhere_marks_the_table_internal`
  already tests it.

  Tracing why the declaration was missing anyway found the real defect, now D-B
  in the proposal: the mint guard in both barrier steps keys on `c.key.name`
  while the reference it pushes carries the full `c.key`. Once one spelling is
  minted, the other resolves to a key nothing minted, and `emit_table_edges`
  drops it with `let Some(target_id) = ids.get(table) else { continue }`. The
  declaration is not mis-flagged; its edge is never written.
- [x] 1.2 Reproduce both defects in a unit test before changing anything, so the
  fix is measured against a red test rather than against a corpus run that takes
  minutes. → verify: one test asserts a qualified and a bare reference to the same
  table reach ONE identity; another asserts a table declared by any of its
  references reports internal.

## 2. D-A — unqualified absorbs into qualified

- [x] 2.1 ~~A qualified reference adopts an unqualified identity of the same
  name.~~ **Implemented, reverted, and superseded by design D1–D3.** The rule was
  not just order-dependent — it answered a question the data does not answer.
  Measured on the corpus, 23 of 158 identities are names with more than one
  spelling, and several carry **two different qualified** spellings
  (`wallets.transfers` and `public.transfers` beside a bare `transfers`). "Which
  schema does the bare one mean" has no answer in the reference.

  Replaced by D2: make the barrier steps two-pass like the `.sql` producer —
  collect every raw reference, decide the identity set, then resolve — which
  removes the order dependence by construction and needs no new policy, since
  `resolve` already grades a bare name matching several as `Ambiguous`.
- [x] 2.1a Implement D2's two-pass identity decision in both barrier steps.
  Blocked on 2.3. → verify: the same workspace yields the same identities
  whatever order its files are walked in, asserted by driving one fixture in both
  orders.
- [x] 2.2 Two *qualified* identities of the same name SHALL NOT merge. →
  verify: a test with `sales.orders` and `archive.orders` keeps two identities —
  the atlas states this explicitly, and merging them would be the worse bug.
- [x] 2.3 **Decided: C.** See design D5 for the measurement and D6 for why two passes are required.

  <details><summary>the options as posed</summary> Under D2, what becomes of a bare
  reference in a workspace that qualifies the same name two ways — 83 bare
  `transfers` references beside `wallets.transfers` and `public.transfers`?

  - **A — fan out**: edges to both, graded `Ambiguous`. Consistent with the
    existing unqualified rule; turns 83 references into 166 edges and lets a
    reader count references that may belong to the other schema.
  - **B — keep a bare identity**: `sql:transfers` survives, meaning "referenced
    without a schema, and we will not guess". Three rows for what may be two
    tables.
  - **C — fan out only when unambiguous** (recommended): adopt the single
    qualified spelling when exactly one exists, keep a bare identity when two or
    more do. The only option that never invents a fact.

  This is about what the atlas should *say*, not about correctness, so it wants
  an answer rather than my picking one. → verify: design.md records the choice
  and its cost.

## 3. D-B — no reference may point at a node nothing minted

- [x] 3.1 Make the mint guard and the pushed reference agree on the identity.
  Whichever way §2 resolves the two spellings, the key a reference carries SHALL
  be the key that was minted. → verify: a test where a qualified and a bare
  reference to one table arrive in *both* orders; each order yields the same
  reference count. Order-dependence is the whole bug, so testing one order would
  be a fixture that never reaches the branch.
- [x] 3.2 Count references whose target is absent instead of skipping silently,
  and surface the count in the run report beside `unparsed`. → verify: the count
  is non-zero on a corpus built with the bug present, and zero after §2/§3.1.
  This is the observability that would have made the defect a one-line report
  rather than a multi-hour trace.
- [x] 3.3 Re-read `emit.rs`'s justification for the skip once it is counted —
  "a missing edge is a smaller wrong than a failed run" stays true, but it must
  no longer read as though absence were expected. → verify: the comment describes
  what the counter now shows.

## 4. Verify on the corpora that found it

- [x] 4.1 The self-index keeps its gains: 29 of 48 tables internal, nothing lost.
- [x] 4.2 On the multi-language corpus, `dealer_users` reports **one** identity
  carrying both its `declares` and its `modifies`. → verify: diff the census by
  `pub_id`, never by name (`fnd_601a3fe6-3fe2-4f5b-a3c1-c9339022a481`) — read by
  name, a pure addition looks like a regression and a real one can hide.
- [x] 4.3 Close `ddl-survives-a-partial-parse` §4.2/§4.3 and §8, which are open
  pending this. → verify: that change's tasks.md points here and its corpus gate
  passes.

## 5. What landed, and what did not

**§2.1 (adoption) was implemented and REVERTED.** A qualified reference adopting
an existing bare identity of the same name is the obvious reading of "a bare name
means schema unstated", and it is unsafe without promotion: nothing records
*which* schema adopted the bare identity, so the next schema adopts it too. On
this workspace's own index that collapsed `sales.orders` and `archive.orders`
into one `sql:orders` — reporting references against a table that never received
them, which §2.2 calls the worse error.

The unit test written to prevent exactly that passed, because its fixture used an
**empty** registry, so the adoption branch never ran. Third instance this session
of CLAUDE.md §9's "suspect the fixture", and the first two were in tests I wrote
minutes earlier for the same change. Two tests now pin the reverted behaviour
with a bare identity present, so neither can pass vacuously.

Unifying the spellings properly needs promotion — the adopted identity becoming
`sales.orders` so a second schema sees a taken name — which rewrites a `pub_id`
already handed out. Left open as §2.3's modelling question.

**What did land is the part that was losing data.** Two changes, each with its
own test, each mutation-checked against that test:

| change | what it fixes | its test |
|---|---|---|
| `Union` — resolve sees what this pass minted | a reference could not reach a sibling minted moments earlier | `neither_spelling_of_one_table_loses_its_reference` (both orders) |
| whole-key mint guard | one spelling satisfied the guard for another; the loser's edge was dropped | `two_schemas_sharing_a_name_both_get_nodes` |

The first single test covered both changes and the mint-guard mutation **survived
it** — `Union` alone made the fixture pass either way. Split into two, each
mutation now kills exactly its own test and nothing else.

## 6. Measured

Self-index and the multi-language corpus, both reindexed with `--force`:

| | baseline | now |
|---|---|---|
| corpus tables | 133 | 158 |
| corpus references | 1014 | **1482** |
| corpus internal | 111 | 135 |
| `refs_dropped` warning | (did not exist) | **absent — zero** |

`dealer_users`, the table that started this, is better than the baseline it
regressed from: baseline `sql:dealer_users` internal with **1** reference (the
`ALTER` was dropped whole); now `sql:users.dealer_users` internal with **2** —
both the `createTable` declaration and the `ALTER`.

+468 references is the size of what was being silently discarded, and +25
identities is the count of spellings that used to collide. **A per-name census
diff is not meaningful across this change** — one name now legitimately maps to
several identities — so the check that matters is references *aggregated by
name*: no name lost references, and no name vanished.

- [x] 4.2 A full per-`pub_id` census diff against the pre-change baseline
  remains impossible — that run did not record `pub_id`s, and re-creating it
  would mean rebuilding and reindexing an old binary for a number no decision
  depends on. The specific claim the task existed to check is verified directly
  instead: `dealer_users` resolves to one identity carrying both its declaration
  and its `ALTER`, compared by identity rather than by name. See §7.
- [x] 4.3 Close `ddl-survives-a-partial-parse` §4.2/§4.3/§8 — its corpus gate now
  passes, so it becomes archivable.

  </details>

## 7. Final measurement, under policy C

The §6 table above was taken under the fan-out rule, before C was chosen. C
replaced it, so the honest final numbers are:

| | pre-change baseline | fan-out (A) | **C, shipped** |
|---|---|---|---|
| identities | 133 | 158 | **147** |
| names split across spellings | — | 23 | **10** |
| references | 1014 | 1482 | **1180** |
| internal | 111 | 135 | **126** |
| dropped references | (unmeasurable) | 0 | **0** |

References fall from A's 1482 to 1180, and that fall is the fix. Under A a bare
reference emitted an edge to every schema of that name, so `transfers` reported
96 + 83 + 48 = 227 references where the sources make 101 — a reader asking how
much code touches `wallets.transfers` was told 96 when 15 of them said so.

Against the true pre-change baseline the count is still up (1014 → 1180),
which is the DDL recovery plus the previously-dropped edges, minus nothing
invented.

4.2's per-`pub_id` check is satisfied differently than written: `dealer_users`
resolves to a single identity (`sql:users.dealer_users`) carrying both its
declaration and its `ALTER`, and it is the *identity* that was compared, not the
name. A full per-`pub_id` census diff against the baseline is still not possible
— that run did not record `pub_id`s — but the specific claim the task existed to
check is verified directly.
