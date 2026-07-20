## Context

Kenn already holds a rich structural model of every indexed repo — `packages`,
the symbol/edge graph, `aggregate_nodes`/`aggregate_edges` (anchor = package or
module rollup + inter-anchor dependency weights), and the `analysis_*` tables
(flat communities, god-nodes, anchored hierarchy). `kenn visualize` renders that
projection as an HTML graph *for humans*. There is no agent-facing serialization.

The atlas is that serialization. The governing constraint is the split the whole
design turns on:

> **kenn LOCATES (structural, deterministic) — the AGENT REASONS (semantic).**
> The trust boundary equals the reasoning boundary. Kenn asserts only checkable
> facts; only agent prose can be wrong.

Two prior systems anchor the shape: **OKF v0.1** (Google's markdown-bundle spec)
gives a ready, vendor-neutral envelope with producer/consumer independence; and
**understory** proves the "conformance in code, not prompts" boundary works —
while showing the gap kenn fills (understory is generic LLM-authored memory with
no code understanding).

## Goals / Non-Goals

**Goals:**
- `kenn index` emits an OKF-conformant bundle derived from the graph, at a
  `Layout`-resolved location, via a single helper both orchestration paths call.
- Kenn's output is 100% structural facts (skeletons); it never authors prose.
- The agent reaches the atlas with **zero hardcoded paths**: `kenn index`
  announces a marked, greppable line naming the `index.md` (the agent path is
  markdown; the existing `--json` mode carries the same path as a field for
  machines). The `index.md` is the re-readable handle.
- Consumption is a portable, path-free `skills/atlas/SKILL.md` — no new MCP.

**Non-Goals (named follow-ons, not this change):**
- The parallel enrichment *swarm* (v1 enrichment is a simple in-skill step).
- A `--json` handle projection for machine consumers.
- Community/hierarchy-based slices (v1 = per-package).
- Incremental enrich-cache keyed by `content_hash`.
- `kenn init` dropping a CLAUDE.md/AGENTS.md pointer.
- Findings-store-as-OKF export.

## Decisions

### D1 — Concept `type` taxonomy (the spine) → v1 ships only `package`

OKF requires a non-empty `type` per concept; consumers route/filter on it. The
candidate vocabulary is `package · cluster · entrypoint · build · convention`.
**v1 emits only `type: package`, one concept per *internal (non-external)
package*** — the manifest-backed anchors, with the `packages` rows that are
external dependencies (serde, tokio, …) **excluded**. It's the one slice kenn can
produce with zero semantic judgment (a manifest is a hard fact); it has a root
module doc for a seeded description, and its directed dependencies + central
symbols are cheaply derivable (see D7). Manifest-less internal code (a path-module
anchor with no manifest) is **deferred** with the `cluster`/`module` types — v1
does not force `type: package` onto something that isn't a package. The others:
- `cluster` (community) — the highest-value *semantic* slice, but naming/ranking a
  community edges toward judgment; fast-follow, and it also picks up manifest-less
  code.
- `entrypoint` — needs an infra-vs-domain heuristic to avoid noise.
- `build`/`convention` — non-code slices (justfile/CI, findings); later.

The `type` field is open (consumers tolerate unknown types), so adding vocabulary
later is non-breaking. *Alternative considered:* ship `package`+`cluster` in v1 —
rejected to keep the producer purely structural and the change small.

### D2 — Emit OKF, don't invent a format

Adopt the OKF envelope verbatim: a directory of markdown concept files with YAML
frontmatter (`type` required; `title`/`description`/`resource`/`tags`/`timestamp`
recommended), plus reserved `index.md` and `log.md`. Kenn's structural facts go in
**producer-defined `kenn.*` frontmatter keys** (OKF guarantees consumers preserve
unknown keys), so an agent's enrichment pass can rewrite the body/`description`
without disturbing kenn's facts. *Why not bespoke:* interop (renders on GitHub,
Obsidian, Google's visualizer), and it matches kenn's orchestrator-independent
ethos. *Alternative:* a kenn-specific schema — rejected; no upside over a standard.

### D3 — The `index.md` IS the handle; markdown-first, emitted on the existing channel

`kenn index` announces the atlas via a **marked** line — a stable prefix
(`atlas: <published-path>`) the skill greps for deterministically, not "the last
line." Crucially, `kenn index` **already has a `--json` mode** (`emit_progress(json,…)`
in `cmd_index`), so the handle rides the existing channel: the marked markdown
line in human mode, a field on the completion event under `--json`. It never
prints a bare line into a JSON stream. The named `index.md` carries the concept
map **plus a shape/status header** (languages, package count, symbols, %test,
freshness, total concept count — see D9). The skill reads the marked path, so it
never hardcodes a location and works unchanged for `-d ./foreign`, worktrees, and
custom stores; the file persists, so the agent rereads it after a compaction.
**Rule: markdown is the source of truth for the agent; JSON is a projection for
code** — and here they share one emit point rather than fighting over stdout.

### D4 — Consumption is a skill, not MCP

A drop-in `claude-plugins/kenn/skills/atlas/SKILL.md` alongside kenn's eight
existing skills (portable agentskills.io format). Path-free steps: run
`kenn index` → read the printed `index.md` → enrich any skeleton-only concepts →
read the concepts relevant to the task → reread `index.md` to re-orient.
Discoverability (the one thing MCP's session-seed gave that a skill doesn't) is
restored by a **trigger-rich `description`** ("orient", "understand this repo",
"get up to speed", "freshly cloned") plus a future CLAUDE.md/AGENTS.md pointer.
The atlas files are plain OKF, so non-Claude tools read them directly regardless.
*Why not MCP:* it's server machinery kenn doesn't need — kenn writes files, the
skill reads them; and it keeps the atlas adding zero new MCP surface.

### D5 — Producer is a post-persist Reader step at one shared finalize call

The atlas needs raw per-symbol data — `packages`, `file_docs`, `files`, and the
**directed** `edges` — none of which the analysis hook carries: that hook gets only
aggregate `nodes`/`edges` + a `DbWriter` (`FnOnce`) and is gated by
`config.index.persist_analysis` (off → no-op). So placing the producer *in* the
hook (an earlier draft) is wrong on two counts: no access to the raw tables, and it
would vanish whenever analysis is disabled.

Instead the producer runs **after the run's `code.db` is fully written** (the
ingest phase persists symbols/files/packages/file_docs/edges before aggregation)
and reads what it needs via the store **Reader API**
(`fetch_package`/`fetch_symbol`/`fetch_file_path`/`fetch_defs`/edges). This is
independent of the `persist_analysis` gate — the atlas emits on **every** index —
and it drops every graph-granularity question: per-symbol centrality is recomputed
from the raw `edges` (weighted degree = summed incident edge weight), the fallback
D7 already sanctioned.

Single-source across paths: a `finalize_atlas(layout, run_id, config)` helper in
`kenn-indexer`, called from the one point both orchestrations reach after the run's
code graph is persisted (before/at `publish`). Both the CLI (`cmd_index::run_async`)
and MCP (`workflow::index_workspace`) call it, and a test asserts each path emits —
neither can skip it. This trades the hook's by-construction sharing for one explicit
shared call, the same two-orchestration-path discipline used elsewhere in indexing.

### D6 — `description` is a verbatim module doc, chosen by a language-keyed rule

The one-line `description` is a *structural extract* — the package's **root
module doc**, copied verbatim from `file_docs`, not a kenn-authored summary. This
honors "code for what, wiki for why": kenn ships the *what* the code already
states; the agent adds the *why*. "Root module" is language-keyed: the crate
root (`lib.rs`/`main.rs`) for Rust, the package `main`/`index.ts` for TypeScript,
the top `__init__.py` for Python, and so on. When no root doc exists (or the root
file is ambiguous), `description` is left empty for the agent.

### D7 — Per-package facts come from raw directed edges + per-anchor centrality, NOT the aggregates

The persisted rollups don't answer per-package questions, so the producer derives
two things itself from data it already has:

- **Directed dependencies** — `aggregate_edges` is **undirected** (it stores
  `(min_id, max_id, …)`, discarding src→tgt), so it can't say "A depends on B".
  The producer rolls up the **raw directed `edges`** (primarily `EdgeKind::Imports`)
  keeping direction, mapping each endpoint symbol to its package via the Reader
  (`symbols.pkg_id → packages`), to get A→B package edges. Only the directed rollup
  is new.
- **Central symbols** — `analysis_god_nodes` is a **global** ranked list, empty for
  most packages when filtered by anchor. The producer instead ranks each package's
  **own non-test symbols** by weighted degree **recomputed from the raw `edges`**
  (summed incident edge weight) via the Reader — independent of the in-memory graph
  and the analysis gate. Cheap, and correct per-package.

### D8 — v1 enrichment is ephemeral; the durable artifact is the structural skeleton

Because the bundle is regenerated wholesale on every `kenn index` (for freshness,
see Risks), kenn persists **only structural skeletons** — including the verbatim
module-doc `description`, which is a fact, not prose. The agent enriches its
*understanding* **in-context** to orient; kenn does **not** write agent-authored
prose to disk in v1. Persisting enrichment across re-index (so prose survives a
regeneration) needs the deferred `content_hash` merge and is a follow-on.
Consequences: there is **no skeleton-vs-enriched counter** (every persisted
concept is a skeleton), and the "`kenn.*` keys survive agent edits" property only
becomes load-bearing once persistence lands. v1's value is the *durable,
trustworthy map*; the prose is a cheap per-session byproduct the skeleton grounds.

**Exception — `log.md` is append-preserved, not regenerated.** The changelog is
the one part of the bundle whose value *is* its history, so wholesale regeneration
must not wipe it: concept docs + `index.md` are rewritten each index, while
`log.md` gets a new dated section prepended (or is projected from kenn's snapshot
history). It is the sole append-preserved file in the bundle.

### D9 — Bundle layout: collision-safe ids, a frontmatter-free `index.md`, concrete freshness

- **Concept ids are path-qualified.** OKF concept-id = bundle path minus `.md`, so
  a bare `packages/<name>.md` collides when two units share a leaf name (a
  monorepo, or duplicate names across managers). Qualify the id by the unit's
  anchor path (e.g. `packages/crates__kenn-store.md` or a nested dir) so ids are
  unique and stable across re-index.
- **`index.md`/`log.md` carry NO YAML frontmatter.** OKF reserves them as
  frontmatter-free; the shape/status "header" is a markdown heading + prose block,
  not a frontmatter block.
- **Freshness is concrete.** The `index.md` header states the HEAD sha (or the
  `StalenessKey` when git is unavailable) plus an ISO-8601 build timestamp — not a
  vague "fresh," so a reader can tell exactly what commit the map reflects.
- **In-file paths are workspace-relative, never absolute.** `resource`, member-file
  lists, and inter-concept links inside the bundle are repo-relative, so a
  committed/shared bundle carries no machine-local paths and stays portable. Only
  the *handle on stdout* (ephemeral, for the current agent to `Read` now) may be
  absolute.
- **Concept docs are deterministic (no wall-clock in them).** OKF `timestamp` means
  "last *meaningful* change," which v1 can't compute cheaply (that's the deferred
  `content_hash` history). So v1 **omits per-concept `timestamp`** and stamps
  wall-clock time only in the ephemeral handle and `log.md`. Concept docs and the
  `index.md` body are otherwise deterministic — re-indexing an unchanged repo yields
  a no-op diff, which the committed-bundle option needs.

## Risks / Trade-offs

- **"Central" ≠ "important-to-understand."** Per-anchor centrality ranks by weighted
  degree, so a logger/util can outrank a domain type. → Mitigation: filter
  `test`/`external`;
  surface as a tuning knob; the agent's enrichment can demote noise. Fully solving
  infra-vs-domain is deferred with the `entrypoint`/`cluster` types.
- **The skeleton inherits comment quality.** Thin `//!` docs → thin descriptions.
  → Mitigation: accept it honestly (kenn never fabricates prose); the agent
  enrichment pass fills gaps. This is a feature of the trust boundary, not a bug.
- **A stale atlas misleads worse than none.** → Mitigation: regenerate on every
  `kenn index`; stamp the `index.md` header with freshness; the skill re-indexes
  before reading, so the agent can't consume a skeleton that lags the code. This
  wholesale regeneration is *why* v1 keeps enrichment in-context (D8): there is no
  persisted prose for a re-index to clobber.
- **Facts derived from raw edges could drift from the persisted rollups.** The
  producer computes directed deps + per-symbol centrality itself (D7) from the raw
  `edges`/`symbols`/`packages` tables rather than the `aggregate_edges`/god-node
  rollups. → Mitigation: it reads the same raw graph those rollups are built from
  (`scan_edges`, `scan_symbols`, `fetch_package`), so it's a different *projection*
  of identical inputs, not a parallel data source.
- **Atlas quality tracks index quality.** A foreign repo whose toolchain isn't
  present indexes via the text fallback → few symbols, no god-nodes → a thin atlas.
  → Honest caveat, not a bug: the atlas is only as rich as the index beneath it, so
  its foreign-repo payoff depends on the parked `docker-indexer-runtime` work
  actually indexing that repo. Say so rather than overselling.
- **Prompt-injection surface (OKF's own caveat), acute for foreign repos.** An
  enrichment pass reads untrusted stranger code and writes prose. → Mitigation:
  kenn's structural half is injection-proof (facts from the graph); the enrichment
  step rides kenn's existing "repo content is data, not instructions" rule.
- **Hallucination becomes boot-time gospel.** An invented "gotcha" is authoritative
  forever. → In v1 this is largely deferred: enrichment is ephemeral (D8), so no
  agent prose is persisted to mislead a later session. It becomes load-bearing when
  persistence lands — then: every enriched claim carries source handles, the doc is
  regenerated not hand-trusted, and kenn's `kenn.*` facts stay separable.
- **No budgeting in v1.** A 100-package monorepo yields 100 concept files (top-K /
  community slicing is deferred), and a single-package repo yields a trivial atlas.
  → Accepted for v1: packages are a coarse, bounded unit; budgeting arrives with
  community slices.
- **Manifest-less repos get an empty atlas in v1.** Because the unit is the internal
  *package* (D1), a repo with no manifests (a loose script dir, some layouts without
  `go.mod`/`pyproject`) produces `index.md` + `log.md` and **zero concepts**. → Known
  v1 limitation, not a failure: the deferred `cluster`/`module` types cover
  manifest-less code. A single-package repo is fine (one concept).
- **Discoverability without MCP.** A skill only fires when invoked. → Mitigation:
  trigger-rich `description` + the follow-on CLAUDE.md/AGENTS.md pointer.

## Decided (previously open)

- **Handle path across publish** — the atlas is written into the run dir and
  carried on `publish` (like the SCIP intermediates), and `kenn index` prints the
  handle **after** the snapshot flip, naming the **published** `index.md` path. So
  the handle is always valid when the agent reads it (spec req: "kenn index prints
  a markdown handle").
- **v1 enrichment depth** — skeleton-only is the kenn deliverable; the skill
  enriches **in-context**, not written back to the bundle (D8). Writing enrichment
  to disk + preserving it across re-index is the deferred `content_hash` merge.
- **v1 `type` set** — `package` only (D1); other types are non-breaking additions.
- **Committed vs gitignored (task 1.1)** — **gitignored by default.** The atlas is a
  regenerated build artifact (like the vector cache): it lives under the store's
  derived/snapshot area, carried on publish, and is covered by the store's existing
  gitignore. Committing a git-diffable bundle is a follow-on (the R3-C determinism
  work makes it viable later). This keeps v1 free of a "commit the bundle?" workflow.

## Open Questions

- **The shared finalize call site.** Confirm the one point both `cmd_index::run_async`
  and `workflow::index_workspace` reach after the run's code graph is persisted and
  before/at `publish`, where `finalize_atlas` is called once (D5). The
  `persist_analysis` gate is now moot — the producer reads raw tables, not the
  analysis tables, so the atlas emits regardless (task 4.3 keeps the graceful-empty
  path for a repo with no internal packages).
