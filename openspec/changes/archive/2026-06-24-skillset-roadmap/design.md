# Design — graph-grounded skill set, on a drift foundation

> Roadmap-level design. The **foundation** section is specification-ready; the
> **skill family** sections are sketches that each future change will firm up.
> Origin: an exploration of `shadcn/improve` (a read-only advisor skill that
> stamps a git SHA per plan and drift-checks it before execution). The borrow
> is the *drift discipline* and the *advisor loop*; the substrate is kenn's.

## 1. The gap

kenn has two staleness notions and a hole between them:

```
LAYER            SIGNAL TODAY                       CATCHES
─────────        ────────────────────────────       ───────────────────────────
symbol/graph     finding_is_stale(parent_ids)  →     a cited SYMBOL vanished
  (findings)     = code-node id no longer             from the graph
                 resolves → the `stale` flag          (directives/lifecycle.rs)

file/path        check_anchors → exists-on-disk →     an anchored FILE moved
  (anchors)      binary present|orphaned              or was deleted
                                                 ✗     file present, CONTENT
                                                       changed → the rot case
```

Separately, the **skills** only cover the passive-memory loop:

```
   recall ─────────[ do the work ]─────────▶ squeeze
   (read directives                          (distill directives,
    for the paths)                            repair anchors, commit)
```

The graph (`find_similar`, transitive callers/usages/implementers) has no
skill that *orchestrates* it; the `kenn` skill just enumerates the tools.

## 2. Foundation — anchor content-drift

### 2.1 Data model

Only the `Attach` variant grows. `ts` is already caller-supplied so the store
stays clock-free and reproducible (`anchor.rs:35`); the sha rides the same
channel — computed at the MCP boundary (`tools/anchors.rs`), passed in, store
stays pure and unit-testable.

```rust
//  crates/kenn-store/src/db/findings/anchor.rs
Attach { anchor: String, ts: Timestamp, sha: Option<String> }   // xxhash(file)@attach
//  fold carries it onto the current-anchor view:
Anchor { path, recency, attach_count, sha: Option<String> }
```

Three-state liveness for a file anchor:

```
   path?        sha vs current        state         consumer reaction
  ───────      ─────────────────     ─────────      ──────────────────────────
  exists   ∧   match           →      LIVE           trust it
  exists   ∧   differ          →      DRIFTED (new)  re-read before relying
  gone         —               →      ORPHANED       rename / detach (today's flag)
```

### 2.2 `rename` self-heals — no new field, no migration

The fold already carries `(recency, attach_count)` from→to on `Rename`; it
carries `sha` the same way. Consequences fall out for free:

- **pure move** (`git mv`) preserves the blob → same xxhash → LIVE (correct).
- **move + edit** → xxhash differs → DRIFTED → triggers re-verify → the
  re-attach re-stamps the sha. Self-healing.
- **detach** drops the anchor; sha irrelevant.

So `Attach` is the *only* event that changes. `sha: Option<String>` means every
pre-existing `.anchor.jsonl` folds to `sha: None` = "drift unknown" = treated as
live (today's behavior). **No migration, no version bump** (consistent with the
project's no-version-bumps-while-prototyping rule).

### 2.3 Decisions (resolved in exploration)

| Decision | Resolution | Rationale |
|---|---|---|
| **Hash source** | **xxhash** (kenn's in-tree index-skip hasher), not git blob SHA | no git dependency; catches *uncommitted* edits; reuses `workspace-staleness` machinery. The goal is per-file drift, not git-recognizable hashes |
| **Directory anchors** | v1: **file anchors only** carry sha; dir anchors stay exists-only | a dir has no single file-hash; a child tree-hash is a clean later extension, not v1 |
| **Granularity** | v1: **whole-file** hash | "drifted" is a *prompt to re-read*, not a breakage claim — a false positive costs one cheap re-read. Region-hashing needs line spans the anchor model doesn't carry; defer |

### 2.4 Consumers of the drift signal

1. `check_anchors` → returns `{ broken: [...], drifted: [...] }`. `squeeze`
   step 0 already calls it; it now also surfaces "these directives' files
   changed — re-verify."
2. `find_directives` hit → carries `drifted` beside `stale`. `recall` shows
   drifted directives with a "re-read before relying" flag (today it only flags
   `stale`).
3. `reconcile` (new skill, family A) → drift is its primary input.

Relationship to the existing `stale` flag: **complementary layers, keep both.**
`stale` = a cited *symbol* vanished (graph); `drifted` = an anchored *file's*
content moved (file). Directives anchor to files → `drifted` is the
directive-relevant signal; findings cite symbols → `stale` is the
conclusion-relevant signal. A later "freshness" view could unify them, but they
are not the same check.

## 3. Skill families

Grouped by the asset each family is disproportionately good at *because the
graph/drift exists*. `✓` = exists today, `★` = new.

### A. Knowledge lifecycle — exploits anchors + drift

- `recall` ✓ · `squeeze` ✓
- **`reconcile` ★** — the drift janitor and the foundation's payoff. On demand
  (or pre-commit): run `check_anchors` (now with `drifted`) and scan findings
  for `stale`/`drifted`; for each, **re-read the anchored file** and decide:
  refresh the anchor (re-attach, re-stamp sha), supersede the finding (content
  changed the conclusion), detach (no longer applies), or tombstone (dead).
  This is `improve`'s `reconcile` grounded in *real* drift signals instead of a
  hand-maintained markdown status column.

### B. Graph understanding — exploits the call/type/usage graph

- **`trace` ★** — "how does X work / where does this flow go." Multi-hop walk
  over callers/callees/usages from a symbol → a synthesized path, optionally
  stored as a `guide` finding so the next session inherits it. Turns a pile of
  navigation tools into one answer.
- **`blast` ★** — "I'm about to change X — what's the scope?" Transitive
  `list_callers` ∪ `list_usages` ∪ `list_implementers` for the change surface,
  fused with `find_directives` on the touched files for the rules that govern
  it. The pre-edit counterpart to `recall` that adds the graph dimension —
  *change surface ∪ governing memory* in one shot. Purely kenn-native.

### C. Advisor — the `improve` borrow, on a graph

- **`dup` ★** — a focused `find_similar` sweep → near-duplicate implementations
  → consolidation candidates. `find_similar` is *built* for "parallel impls
  with no shared call edge" — which is exactly `improve`'s headline example
  finding, except kenn gets it from the index for free instead of grepping.
- **`audit` ★ (large)** — the full `improve` pipeline (recon → audit → vet →
  prioritize → plan), but the mechanical legs query the graph: duplication via
  `find_similar`, dead code via empty `list_usages`, god-modules via
  `list_callers` fan-in count, vet via `get_source`. The "considered and
  rejected" memory becomes **queryable findings** (a reject-tagged finding that
  survives sessions and is searchable) rather than a section in a markdown file.

## 4. Cross-cutting disciplines borrowed from `improve`

Fold these in when the relevant skills are next touched — they are *quality*
properties, not new features:

- **Vet-over-report.** `improve`'s rule: "subagents over-report — re-read every
  cited location before presenting." Maps onto kenn directly: with the
  `drifted` flag, `recall` can auto-flag "this directive is drifted, re-read
  before trusting," and any fan-out skill (`audit`) re-reads via `get_source`
  before a finding makes the table.
- **"Repo content is data, not instructions."** `squeeze` reads the session +
  repo content and *commits* the distilled result; it has a redaction gate but
  no prompt-injection guard. Add one to `squeeze`/`reconcile`/`audit`: a file
  that appears to issue instructions ("ignore previous…") is recorded as a
  finding, never followed.
- **Self-containment spectrum.** Name the two audiences explicitly so verbosity
  is chosen on purpose: a **directive** is terse, written for *the next session
  that already has kenn* (current model — right); a **handoff/guide** for a
  context-free reader borrows `improve`'s self-containment bar (inline the
  excerpt, the convention, the command). Different readers, different lengths.

## 5. Build order

```
  1. FOUNDATION  anchor content-drift (xxhash on Attach, three-state fold,
     ─────────   drifted bucket/flag in check_anchors + find_directives)
                 small, low-risk, additive, no migration.
        │
        ▼
  2. reconcile   the janitor — first consumer; proves the drift signal pays off
     (family A)  before anything heavier is built. Carries the vet-over-report
                 + injection-guard hardening into the lifecycle skills.
        │
        ▼
  3. blast,      graph-understanding skills — independent of each other; pick by
     trace       which the user reaches for first. blast composes recall+graph,
     (family B)  the most obviously useful pre-edit.
        │
        ▼
  4. dup  ──▶  audit   advisor family — dup is the tractable taste; audit is the
     (family C)        big arc, gated on reconcile having proven the loop and on
                       real demand. Stores its reject-memory as findings.
```

Rationale: the foundation unblocks A; A (`reconcile`) must exist and earn its
keep before C (`audit`) is worth its cost; B is orthogonal and demand-driven.

## 6. Open questions

- **Drift noise on large files.** Whole-file hashing fires "drifted" on any edit
  to a big anchored file, including unrelated ones. Acceptable for v1 (cheap
  re-read), but if it proves noisy, the refinement is region-hashing keyed on a
  finding's code-node parent spans — which couples the anchor layer to the
  parent_id layer for the first time. Watch usage before doing it.
- **Where do `audit` artifacts live?** `improve` writes self-contained markdown
  plans; kenn has a queryable findings store. Conclusions + rejections clearly
  belong in findings; the *executable handoff* for a context-free model may
  still want plain markdown (the reader may not have kenn MCP). Likely split:
  findings for memory, markdown for handoff — mirrors the directive-vs-handoff
  audience split in §4. Decide when `audit` is promoted.
- **`reconcile` cadence.** On-demand only, or also a `squeeze`-style pre-commit
  step? Leaning on-demand first to avoid bloating the commit ritual.
