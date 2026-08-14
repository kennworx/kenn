## Why

A finding records one of two very different things, and the store treats them
identically.

A **rule** says how this codebase works: "shell-safety flooring is per-ingester",
"never spawn through `/usr/bin/env`". A rule is about intent. When its anchored file
changes, the rule almost always still holds — the drift is incidental.

A **claim** asserts the current state of the code: "the producer emits spurious
zero-range defs", "`get source` returns only the declaration line", "this CRAP entry is
legitimate debt". A claim is about facts, and facts move. When its anchored file
changes, the claim may simply have stopped being true — and nothing notices, because
the person who fixed the code had no reason to look for a finding that described it.

Both failure modes were hit in one session:

- A finding recorded a deferred producer bug with "remaining work: stop emitting
  zero-range defs". Its successor declared it FIXED by a store-side `DELETE`. Neither
  was accurate: the `DELETE` fixed only the shadowing case, and the residue it could not
  reach turned out to be load-bearing. Acting on the record's "remaining work" removed a
  placeholder that is the symbol-to-file link for aggregation. A test caught it.
- A finding recorded `Kind::db_name` as a legitimate CRAP grandfather that must not be
  "contorted into a table-driven scan". The reasoning was sound and its conclusion had
  been overtaken — a derive preserves the exhaustiveness the rule was protecting, which
  the finding could not have anticipated.
- Separately, a CRAP baseline entry named a function of cyclomatic 1, having recorded a
  different function that an unrelated refactor had already brought under threshold. It
  had been carrying phantom debt with nothing to notice.

The store has the raw signal — `check findings` computes drift, and
`findings directives` already returns a `drifted` flag per item. What it lacks is the
distinction that makes the signal actionable. Today an agent reading a drifted claim
sees the same thing it sees for a drifted rule: nothing that says "this was true once".

## What Changes

A finding declares whether it is a rule or a claim. Claims carry an obligation rules do
not: once the code they describe has changed, a claim SHALL be re-verified before it is
acted on, and the surfaces that serve findings SHALL say so rather than presenting it as
current fact.

`kenn check findings` gains a bucket for drifted claims, distinct from the anchor-repair
buckets it reports today — those ask "does this file still exist"; this asks "is this
still true".

The before-commit ritual gains a matching step: re-verify the claims your changed paths
carry, the same way it already re-confirms the directives they carry.

## Impact

- Affected specs: `findings-lifecycle`
- Affected code: `crates/kenn-store/src/db/findings/`, `kenn check findings`,
  `kenn findings directives`, the `kenn:squeeze` skill
- Existing findings are unaffected until tagged; the classification is additive and the
  absence of a claim tag means "rule", which is the safe default for a store whose
  entries are overwhelmingly rules.
