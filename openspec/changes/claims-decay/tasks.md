## 1. Classification

- [x] 1.1 Mark a finding as a claim through the existing tag vocabulary rather than a new
  column — the store already carries `bug`, `deferred`, `gotcha`, `fixed`, and
  `usability` on exactly the findings that decay, and `directive`/`guide` on the rules.
  Read the classification from tags so existing findings classify correctly with no
  migration and no re-tagging pass. → verify: the two incidents that motivated this
  (`fnd_7fcaaa59` zero-range defs, `fnd_2d7e042b` get-source) classify as claims from
  their existing tags; a `directive` with no claim tag classifies as a rule.
- [x] 1.2 Default an unmarked finding to "rule". → verify: a finding with no tags is not
  reported unverified when its anchor drifts (spec scenario).

## 2. Reporting

- [x] 2.1 Add an `unverified` set to `kenn check findings`, alongside `broken` and
  `drifted`, holding claims whose anchored content changed since the claim was recorded.
  Do NOT fold it into `drifted`: the existing buckets ask whether a path still exists and
  whether bytes moved; this asks whether an assertion is still true, and merging them is
  what made the signal unreadable at 127 entries. → verify: a drifted claim appears in
  `unverified` and a drifted rule does not (spec scenarios).
- [x] 2.2 Exclude superseded ancestors from every bucket. Measured on this repo, 26 of
  127 drifted entries were ancestors that `findings directives` already excludes from
  retrieval, so they can never surface as guidance — reporting them is 20% pure noise
  that trains readers to skim the list. → verify: a superseded finding is absent from
  `broken`/`drifted`/`unverified`.
- [x] 2.3 State nothing about whether an unverified claim still holds. → verify: the
  report carries no resolved/unresolved judgement for an unverified claim (spec
  scenario).

## 3. Serving

- [x] 3.1 Mark verification status on claims returned by `kenn findings directives` (and
  the MCP equivalent), so a consumer acting on a claim can tell a confirmed one from one
  that predates the current code. The `drifted` flag already rides on each item; this is
  a distinct field, because drift on a rule is not a warning and drift on a claim is. →
  verify: a claim whose code changed is marked unverified in the response; one confirmed
  against current code is not (spec scenario).

## 4. Verification outcomes

- [x] 4.1 Add a verification outcome to `kenn findings touch` — still true, no longer
  true, partially true — separate from `attach`. → verify: the outcome is recorded and
  readable.
- [x] 4.2 Do NOT let `attach` clear the unverified mark. `attach` means "this applied to
  my change" and refreshes the content hash; letting it double as verification is the
  silent failure this change exists to remove, and it would let a bulk re-attach declare
  a store's worth of claims true without anyone reading one. → verify: attaching an
  unverified claim leaves it unverified (spec scenario); mutation — make `attach` clear
  the mark, confirm the test goes red, restore.
- [x] 4.3 Record "no longer true" by superseding with a finding describing the current
  state, reusing the existing `supersedes:` lifecycle rather than a delete. → verify: the
  superseded claim stops being served (spec scenario).
- [x] 4.4 Express a partial fix as a distinct outcome. The motivating failure was a
  successor asserting FIXED where the fix covered only the shadowing case, leaving a
  residue that read as untouched outstanding work — and acting on that reading removed a
  load-bearing placeholder. → verify: a partial outcome distinguishes the fixed part from
  the residue (spec scenario).

## 5. Ritual

- [x] 5.1 Add a claims step to the `kenn:squeeze` skill: for the changed paths, re-verify
  the claims they carry, as the ritual already re-confirms their directives. This is the
  step that would have caught both incidents — each was a claim anchored to a file the
  fixing change touched. → verify: the skill names the step and the outcomes it accepts.
- [x] 5.2 Say plainly in the skill that a drifted rule usually still holds and a drifted
  claim may not, so the two are not worked through with the same effort. → verify: the
  guidance distinguishes them.

## 6. Verification

- [x] 6.1 Replay both motivating incidents against the implementation: the zero-range-def
  claim and the get-source limitation should both have surfaced as unverified at the
  moment their code changed. → verify: each appears in `unverified` when replayed against
  the commit that changed its anchored file.
- [x] 6.2 Measure the unverified set on this repo and record it. If it is large, the
  classification is too broad and 1.1 needs narrowing — the value of this change is a
  list short enough to actually read. → verify: the count is recorded alongside the 127
  drifted / 26 superseded figures this change was designed from.
- [x] 6.3 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering).

## What verification found

**3.1** — `FindingHit.unverified`, a field distinct from `drifted`. The two mean opposite
things: a drifted *rule* ("do not do X") is almost never made false by its file moving on,
while a drifted *claim* ("X is broken", "Y is fixed") may have been made false by that very
change and is still served as fact. Folding them into one flag is what let both incidents
through. No hit carries both.

**4.1** — three ops on `kenn findings touch`: `verified`, `stale`, `partial`. Each writes
an `AnchorEvent::Verify` carrying its own sha and outcome; all three round-trip through the
committed log format.

**4.2 — made structural rather than remembered.** `attach` and `verify` write different
fields, and a claim is measured against its *verification* sha, falling back — when never
verified — to its **origin** sha, the content the claim was written about, never the latest
attach.

That fallback is where the first implementation was wrong. Pointing it at the latest attach
sha reintroduced exactly the hole this task forbids, and the replay test caught it on its
first run. Two mutations now go red: making `attach` record a verification, and pointing
the fallback back at the latest attach.

**4.3** — `stale` records the reading; retirement stays the existing `supersedes:` path,
and superseded findings are already excluded from every health bucket and from retrieval.
A claim read as no-longer-true **stays flagged** until superseded: knowing the answer is
"this is wrong" is not a resolved state.

**4.4** — `Outcome::PartlyTrue` is its own variant, and the skill tells the reader to reach
for it rather than round to verified, naming the incident as the reason.

**5.1 / 5.2** — step 0 of `kenn:squeeze` now reports three buckets, gives the three commands
with when to use each, and says outright that "a drifted rule is usually a glance, an
unverified claim is a read".

**6.1** — replayed as a test of the shared *shape* rather than against frozen repo state,
since both incidents were the same shape: a claim tagged `fixed`, anchored to a file, made
wrong by a later change to it. The full lifecycle is asserted in order — the claim surfaces
when its code changes, a bulk `attach` does **not** clear it, a read does, and the next
change puts it back. A second test pins that a `stale` reading stays flagged.

Measured on this repo afterwards: **217 findings, 9 claims, 3 unverified** — small enough
to act on, which is what 6.2 asked for. It also confirms the unmarked-means-rule default:
172 directives and 50 guides against 9 claims.
