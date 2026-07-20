# Design — agent-directives

## The governing principle: kenn is not intelligent

The whole design follows one cut: **kenn stores / retrieves / guides; the agent
reasons.** Anything that requires judgement — is this turn a directive? what is
the distilled rule? does this code violate it? which anchor should it pin to? —
is the agent's job, performed via a skill. kenn only does deterministic things:
persist a record, embed prose, fold an event log, match a path, fuse two ranked
lists, report whether a path resolves. This keeps kenn portable across agents and
keeps the surface small.

A corollary: the *embedding model* kenn runs is **not** a violation of this
principle — text→vector is a deterministic transform, like an index. A
*generative classifier* inside kenn would be a violation, so classification of
directions/judgements is the agent's job. (If transcript volume ever demands a
cheap first-pass triage, anchor-similarity over the existing embedder — k-NN to a
few labeled anchor phrases — is mechanical and would be acceptable; it is not in
scope here and is not needed at current scale.)

## Fit with kenn's established philosophy

Two existing requirements in `findings-mcp` constrain — and validate — this
design, and the proposal is built to extend them, not work around them:

- **"The MCP server runs no model and performs no task analysis."** The new tools
  (`find_directives`, `check_anchors`, anchor-event writes) are primitive —
  path-anchored search, path-existence checks, finding writes. None interpret a
  task or plan work. The intelligence (classify / distill / judge / redact) is
  the agent's, via the workflow. This is the same "kenn is dumb" line, already law.
- **"A system-prompt fragment drives finding accumulation," orchestrator-
  independent.** Guidance already ships as a portable, installable fragment — not
  per-orchestrator glue. So the recall + before-commit ritual **extends that
  fragment** (one source of truth, works for any agent), and the Claude-Code
  plugin (`squeeze` / `recall` skills, guide) is a *surfacing* of it, not a
  parallel reinvention. The earlier draft put the workflow only in plugin skills;
  that would have stranded non-Claude orchestrators and duplicated the mechanism.

The squeeze ritual is also compatible with the documented **subagent-as-extractor**
pattern: a squeeze MAY run as a dispatched subagent that records directives and
returns their ids, coordinating through the store like any other extractor.

## Routing: constitution / case-law / personal notes

Three homes for steering knowledge, chosen by *sharing scope* and *load
semantics*, not by topic:

- **CLAUDE.md = constitution.** Few, foundational, always in force, must stay
  small (it loads every turn). Claude-specific.
- **kenn directive = case law.** Many, specific, cited when the situation
  matches (file/dir proximity + semantics). Team-shared, agent-agnostic,
  relevance-loaded.
- **feedback_*.md = personal notes.** A developer's private working style, this
  machine only.

The squeeze skill applies the routing rule:

```
  shareable with team?  ──no──▶  personal note (unchanged, out of scope)
        │yes
  must apply every turn, small/universal?  ──yes──▶  CLAUDE.md
        │no  (context-specific, or would bloat the constitution)
        ▼
  kenn directive  — pin to the code it's about; retrieved when relevant
```

## Mutability decides the home

```
  IMMUTABLE → md (frontmatter + body)      MUTABLE → <id>.anchor.jsonl
  ───────────────────────────────────      ──────────────────────────────
  id, created_at                           which files/dirs it applies to
  tags (directive|guide, polarity:*)       liveness: when re-attached
  parent_ids (provenance — to change,      rename / move / delete fixups
    make a NEW finding that supersedes)
  the knowledge prose (why / how)

  to change → new finding (supersede)      to change → append an event
```

`parent_ids` point backward at provenance that already happened — immutable by
nature; you cannot un-derive a conclusion. Anchors point forward at files/dirs
that keep moving. Putting a churning reference inside an append-only document is
a category error, so anchors leave the frontmatter entirely.

## Anchors and liveness as an append-only event log

`<id>.anchor.jsonl`, one finding's anchors + heartbeat, folded to current state.
Event kinds are **`attach` / `rename` / `detach`** — there is no separate "fire"
or "confirm": a re-`attach` to a path already in the set *is* the liveness signal
(it bumps recency and contributes to relevancy). Events carry a `ts` only — **no
commit hash**: the squeeze appends events *before* the commit exists, and the
jsonl is itself part of that commit, so a current-commit hash is unknowable at
write time.

```jsonl
{"op":"attach","anchor":"crates/kenn-mcp/src/server.rs","ts":"2026-06-01T…"}
{"op":"attach","anchor":"crates/kenn-mcp/src/server.rs","ts":"2026-06-07T…"}
{"op":"rename","from":"…/server.rs","to":"…/mcp_server.rs","ts":"…"}
{"op":"detach","anchor":"indexers/kenn-dotnet/","ts":"…"}
```

- **Current anchor set + per-anchor liveness = fold over the log.** Recency =
  the latest `attach` ts. Relevancy = a **recency-weighted** attach frequency,
  not a monotonic lifetime count — recent re-attaches count more and an anchor
  that stops being re-attached decays, so a once-hot but now-obsolete directive
  does not stay entrenched at the top until it is superseded.
- **Mergeable by construction:** different findings → different files; mutation
  is an appended line, never an in-place edit. Concurrent appends to the *same*
  finding's log are the only conflict, and the resolution is the union of lines.
- **Liveness is earned through use, but liveness ≠ validity.** A directive
  re-attached as related code keeps changing stays live. The trap: a directive on
  *stable, foundational, rarely-edited* code (often the most important
  invariants) never appears in a diff, so it would decay despite being perfectly
  valid — absence of churn is not evidence of obsolescence. Therefore decay SHALL
  only **lower retrieval rank**, never auto-retire. Retirement (supersede /
  tombstone) is always **agent/user-judged**, prompted by a contradiction or an
  explicit review, not by a liveness threshold.
- **Guard the rich-get-richer loop.** `find_directives` ranks by liveness and the
  ritual re-attaches what it surfaces, which boosts liveness — a feedback loop
  that can entrench early directives and bury newer, equally-relevant ones. The
  *attach-on-confirmed-relevance* gate is the brake (the agent only re-attaches
  what genuinely applied), and the recency-weighting decays unreinforced lead.
  Retrieval SHOULD also surface a few low-liveness anchor matches so new
  directives are discoverable, not only the incumbents.
- **Attach on *confirmed* relevance, not on retrieval.** Vector search surfaces
  marginal matches; auto-attaching them would inflate liveness with noise. The
  agent (or user) judges that a directive actually applied before an `attach`
  event is appended.

## Retrieval is by file/dir (v1 anchors are paths)

The guide steers the agent to **search by the file(s)/dir(s) it is touching**,
and v1 anchors are file/dir paths to match — because:

- the before-commit ritual already starts from the diff = files + dirs;
- files/dirs are the stable, agent-obvious granularity (no extra
  `find_at_location` hop);
- a coarse dir anchor already covers fine-grained work under it;
- symbol anchors would be *less* stable than files (node ids move/vanish on
  reindex) and would drag the index dependency into `check_anchors` — so they are
  deferred. When symbol-level matters, reduce it to "find the symbol's file,
  search by that."

```
  find_directives(paths):
    leg 1  anchor exact-path + ancestor-dir match        (structural)
    leg 2  body-vector proximity                          (semantic)
    ⊕ RRF-fuse the two ranked lists (reuse kenn-store fusion)
    then reweight by recency-weighted liveness (decayed → lower rank, not dropped)
```

**Degrade to anchor-only when the embedder/index is not ready.** The semantic leg
needs the embedder and snapshot; those return `EMBEDDER_STARTING` /
`INDEX_UNAVAILABLE` / `EMPTY_SNAPSHOT` (all `-32002`) before they are warm. But
anchors are *committed files* — resolvable with no index at all. So
`find_directives` SHALL fall back to the structural (anchor) leg and still return
results rather than erroring. This is not just robustness: directives work
*before* the first index, which matters for a fresh clone.

**Creating an anchored directive is one call.** `store_finding` gains an optional
`anchors` list so the squeeze creates a directive and its initial anchors
together; re-anchoring later (the liveness path) is a separate append. Without
this the squeeze would need two calls per new directive.

## Orientation: file with a tool fallback (no resources)

MCP resources were considered and dropped. Per the MCP spec a `resources/read`
*may* return `-32002`, but resource handling is "application-driven" and a
passive, host-managed read that errors during indexing is silently swallowed by
many clients — whereas a *tool* error lands in the agent's turn where it can
retry. Instead, `kenn index` materializes a run-local `overview.md` (in the
active `runs/{id}/`, reached via `live` — runs-centric, no snapshots dir); the
agent reads it with the tool it already trusts. The binary "file exists / file
absent" *is* the readiness signal: absent → guide says call
`get_index_status` / `get_workspace_overview`. Snapshot-local and derived, so it
is never committed and each machine regenerates its own.

## The collect layer already exists: conversation-history-store

The original framing was "**collect** [user directions], then **batch process** by
an agent to squeeze it, e.g. before commit." The collect half is already built:
`conversation-history-store` captures, via short-lived `kenn cc-hook` processes
(no LLM, machine-local `collector.db`), exactly what the squeeze needs —

- **which files were touched**, keyed by `project` *and* `branch` (so "what was
  worked on this branch before this commit" is a query, broader than the staged
  diff);
- the user's **prompts** (`sessions.last_prompt` + prompt events) — the raw
  directions to distill;
- `transcript_path` — a pointer to the full session JSONL for deeper mining.

So the squeeze does **not** introduce a new capture mechanism. It reads
`collector.db` (branch-filtered touched files + prompts) and, when needed, the
transcript, then distills. This also makes the privacy boundary crisp and is
exactly where the redaction gate sits:

```
  collector.db (machine-local, raw, has prompts/paths)   ← collect (exists)
        │  squeeze: agent distills + redacts
        ▼
  committed directives (team-shared, redacted)            ← the team store
```

Anchoring stays precise (to the staged diff's files — what's actually committed);
the *directions* and broader session activity come from `collector.db`. No spec
of `conversation-history-store` changes — the squeeze is a consumer.

## The before-commit ritual (guidance, not a hook)

```
  0. check_anchors            → fix moves/deletes from this diff
  1. PULL  find_directives(changed files/dirs) → agent judges relevance
           → guardrail: does the diff violate a directive? warn
  2. RE-ATTACH the confirmed-relevant ones (append `attach`) → liveness bump
  3. SQUEEZE this session's directions/corrections → distill → new
           directives, anchored to the changed files/dirs (supersede if a
           directive changed)
```

No git hook: the ritual is words in the guide. Mining favors **corrections and
recurrence** (strong, easily detected) over explicit praise (rare, ambiguous).

## Safety: directives are committed and team-shared

Squeezing user turns into committed files crosses a privacy boundary the manual
memory system never did (that store is machine-local). A mined directive can
carry secrets, absolute paths, or private/customer identifiers that must not land
in a shareable artifact. So the squeeze SHALL apply a redaction gate before
writing: strip credentials and machine-local paths, and never write private
project/customer names into a committed directive. When in doubt the agent asks
rather than commits — this is exactly the routing rule's "personal note"
fallback, which keeps the unshareable in machine-local `feedback_*.md` instead of
the team store.

## Orientation freshness: present ≠ fresh

"File exists = ready" answers *built at all*, not *built recently*. The watcher
can lag (a known issue), leaving a stale-but-present `overview.md`. So
`overview.md` SHALL carry the snapshot's `indexed_at`, and the guide tells the
agent to treat a far-past `indexed_at` as a prompt to check `get_index_status` /
reindex — not to trust presence alone.

## Scope and sequencing

This change is large (4 capabilities + a storage-format migration). It is kept as
one change but **sequenced** so the riskiest, most independent piece lands and is
verified first:

1. **Finding record `json → md`** (findings-store, store-layout) — touches the
   derived-store rebuild parser and every reader; independent of directives.
   Land and verify green before anything below.
2. **Anchor sidecar + liveness** (findings-store) — on top of the md records.
3. **Tools** `find_directives` / `check_anchors` / anchor writes (findings-mcp).
4. **Agent-guide layer** (agent-guide) — skills + guide, last.

Migration mechanism: a one-shot converter on first open — read legacy
`<id>.json`, write `<id>.md` with the body **byte-for-byte equal to the prior
`text`** (so the fingerprint-keyed committed embedding reuses with no re-embed),
then remove the legacy `.json` so there is a single source of truth. Legacy
records have no anchors, so no `.anchor.jsonl` is created at migration. Leans
one-shot over dual-read — there is one legacy record and the project changes
shapes in place while prototyping.

## Directive vs guide — distinct roles, not just two tags

The two reserved tags are not interchangeable: a `directive` is a **rule** (it
carries `polarity:do` / `polarity:dont` and is what the before-commit guardrail
checks the diff against), while a `guide` is **orientation / how-to context**
(retrieved alongside directives, but never violation-checked and carrying no
polarity). `recall` presents them distinctly — rules grouped by polarity, guides
as context — so the agent knows which it must not violate versus which merely
informs. Keeping both in v1 costs nothing (same store, same retrieval) and the
guardrail seeds specifically off `polarity:dont`.

## Supersede must carry anchors forward

The base store says retrieval *prefers the latest finding in a supersede chain*.
But a superseding finding gets a fresh, empty `anchor.jsonl` — so the corrected
directive, though preferred, would have **no anchors and be unreachable by
`find_directives`** (which is path-anchored). So on supersede the successor's log
is **seeded with the predecessor's current anchor set**. Liveness resets to "now"
(a correction is a fresh, confirmed statement), but reachability is preserved.
`find_directives` likewise inherits the supersede/tombstone exclusion — it is
retrieval, so it prefers the latest in a chain and drops tombstoned directives.

## Honest limitations

- **The guardrail catches `dont` better than `do`.** A `polarity:dont` violation
  is a forbidden pattern *present* in the diff — detectable. A `polarity:do`
  violation is a required step *absent* from the diff — an omission, much harder
  to spot. The guardrail seeds off `polarity:dont`; `do`-rule enforcement is
  best-effort.
- **Value depends on the agent running the ritual.** No hook means `recall`
  (start of work) and `squeeze` (before commit) fire only when the guide prompts
  the agent and it complies — the same opt-in caveat as any guidance-not-hook
  design. Accepted deliberately (a hook can't carry the judgement the steps need).

## Inconsistencies resolved in this change

Because the change already MODIFIES "the findings derived store and embeddings
are derived" (it names the record format), it fixes two pre-existing snags there
rather than leaving them dangling:

- **Findings vector path.** `findings-store` had `.kenn/findings/vectors/` while
  `store-layout` (the layout authority, asserted by its default-layout scenarios)
  has `.kenn/vectors/findings/`. The MODIFIED requirement now uses
  `.kenn/vectors/findings/`, so the change is internally consistent.
- **"fingerprint of its text" → body.** The derived-embeddings requirement keyed
  the embedding by "the fingerprint of its `text`"; with md records that is the
  **prose body**. The MODIFIED requirement says so. Migration keeps body == prior
  `text` byte-for-byte, so existing fingerprints and committed embeddings are
  preserved (no re-embed).

## Two freshness notions, kept distinct

A directive has two independent "is this still good?" signals, and conflating
them would mislead:

- **`stale`** (read-time, from `parent_ids`) — the *evidence* moved: a code-graph
  node the directive was derived from no longer resolves. `find_directives`
  returns it **marked, not omitted** (inherits the base staleness rule).
- **unresolved anchor** (from `check_anchors`) — the *place it applies* moved: an
  anchored file/dir was renamed/deleted. Repaired with a `rename`/`detach` event.

A directive can be anchor-valid but stale (evidence gone, location intact) or
anchor-broken but not stale, so the two are reported separately.

## Open: directives in generic search

Directives/guides are findings, so they also surface in `semantic_search` /
`search_findings`. Leans **include** — they are legitimate knowledge and the
`tag:directive`/`tag:guide` filter lets a caller exclude them; `find_directives`
is the targeted, path-anchored path. If generic-search noise proves a problem,
de-rank (not drop) tagged findings in generic results. Parked as a tuning detail.

## Notes

- **`overview.md` vs `get_workspace_overview`** — two surfaces for the same data
  risk drift; they SHOULD share one generator, so the file is the cached form.
- **`overview.md` is not a schema-versioned snapshot artifact** — it is a
  regenerated, human/agent-readable convenience doc in the run dir, outside the
  version-checked store snapshot; a reader treats it as prose, not typed data.

## Adjacent capabilities checked (no change needed)

- **`mcp-server`** — `tools/list` is pagination-conformant and explicitly holds
  "for future growth"; adding `find_directives` / `check_anchors` / anchor-write
  asserts no fixed tool count. The server advertises tools-only capability — and
  this change adds no MCP resources, so that stays correct.
- **`incremental-embedding` / `embeddings-api` / `embedding-producer`** — the
  findings vector sidecar reuses the incremental-embedding format keyed by the
  body fingerprint; embedding *input* is text (the body is text). Migration keeps
  body == prior `text`, so fingerprints and committed vectors are unchanged.
- **`indexing-orchestrator` / `kenn-server`** — `overview.md` is an additional
  run output written by the index pass; no existing requirement enumerates an
  exhaustive run-output set that it would violate.
- **`conversation-history-store`** — consumed, not modified (see "The collect
  layer already exists").

## Decisions parked for implementation

- **Record-anchor as a tool vs. direct file-append.** A tool keeps the derived
  index / liveness rollup consistent on write; a direct append is zero-surface
  but leaves the rollup to be rebuilt. Leans tool.
- **`check_anchors` rename suggestions.** It may annotate a broken anchor with a
  git-rename-detected candidate (git rename detection is itself mechanical); the
  agent still decides. Leans include-as-hint.
- **Anchors as a readable frontmatter cache.** Whether to also project the
  current anchor set into the md frontmatter for at-a-glance reading, accepting
  it is a derived cache of the jsonl. Leans no (avoid a cache that can drift;
  the jsonl is small and readable).
- **Garbage-in gate.** Whether a mined directive goes live immediately or stays
  staged until it recurs / the user confirms via the skill's "ask." Leans
  confirm-on-first-capture, auto-live on recurrence.
