---
name: kenn
description: Navigate, search, and understand an indexed codebase with the kenn CLI — find symbols, callers, implementers, and usages; trace scope and imports; semantic search over code; and record or reuse provenance-tracked findings. Use whenever exploring or answering questions about the structure of a codebase that has a kenn index.
---

# kenn — code graph over an indexed codebase

kenn indexes a workspace into a **code graph** — symbols, calls, types, and
scopes across the whole project — plus a **findings store**, a durable,
provenance-tracked memory of conclusions. Reach for kenn instead of grepping
when the question is about *structure*: who calls this, what implements that,
where a symbol is defined, what a module contains.

Drive it through the **`kenn` CLI** (run commands with the Bash tool). Every
command prints **TOON** by default (a compact, skimmable table); add `--json`
for JSON you can pipe to `jq`.

> The same operations exist as MCP tools (`find_symbol`, `list_callers`, … —
> the CLI verbs map 1:1) if the kenn MCP plugin happens to be loaded. This
> skill uses the CLI; it needs nothing but the `kenn` binary on `PATH`.

## Getting started

`kenn status` reports the live snapshot and counts. If there is no snapshot,
run `kenn index` once to build it. `kenn overview` gives the orientation view —
languages, package/file/symbol counts, graph shape.

Structural commands (`kenn find symbol`, `kenn list callers`, …) work as soon as
the index exists. The **vector** commands — bare `kenn find <query>` (semantic
search) and `kenn find similar` — need embeddings; the CLI loads the embedding
model on first use (a few seconds), so the first vector command in a run is
slower. If `kenn overview` shows no embedder, vectors are unavailable and those
two commands degrade to empty/lexical.

## Commands

**Search — find a symbol**
- `kenn find symbol <name>` — literal lookup (exact→prefix→fuzzy), each row
  tagged with why it matched.
- `kenn find symbols <query>` — ranked full-text search over names + doc
  comments.
- `kenn find <query>` — **semantic** search over code *and* findings; best when
  you don't know the exact name. `--scope code|findings|both`.
- `kenn get symbol <id>` — full detail for one symbol id.
- `kenn find at-location <file> <line>` — symbols covering a `file:line`,
  smallest-enclosing first; use for stack traces.

**Navigate — follow the graph** (each takes a symbol id)
- `kenn list callers <id>` / `kenn list callees <id>` — the call graph.
- `kenn list implementers <id>` / `kenn list overrides <id>` — interface/trait
  impls, method overrides.
- `kenn list usages <id>` — every reference to an id, tagged with its edge kind.
- `kenn find usages <query>` — **one-call "where used"**: takes a name / path /
  `pub_id` (not just an id) and returns incoming references directly. Use this
  for the common case instead of a two-step lookup. Empty = used nowhere (not an
  error). An ambiguous query resolves several targets and returns `next: null`
  (no paging) — narrow with `--kind`/`--path`/`--package` or pass a `pub_id`.
- `kenn list correspondences <id>` — links across languages or decl/def.
- `kenn find similar <id>` — symbols nearest a given symbol's committed vector
  (look-alike logic with no shared call edge). Reuses stored vectors, so it
  needs no model load.

**Scope — what is inside**
- `kenn list in-scope <id>` — direct children of a module or type.
- `kenn list imports <id> --direction outbound|inbound|both` — module deps.
- `kenn list module-files <id>` — the files of a module.

**Orient — the structural axes**
- `kenn overview`, `kenn status`.
- `kenn packages [<name>]` — every package with its role (provider / layer /
  consumer / tests / isolated) and coupling counts, most-depended-on first. With
  a name: its typed coupling both directions, its root doc, and its most
  connected symbols.
- `kenn domains [<query>]` — clusters that span more than one package: the
  structure the package list can't show. With a name (hub id or title): the
  packages it spans and its central symbols.
- `kenn contracts [<query>]` — interfaces / base types implemented in more than
  one package, widest span first. With a name: every implementer grouped by
  package, each with a resolvable id. An EMPTY result is a real answer, not a
  failure: Rust and Go keep abstractions package-local, so their contracts axis
  is legitimately empty, while C# and Swift spread them across the tree.
- `kenn documents [<name>]` — the non-code directories kenn tracks (docs,
  specs), with file counts.

A name argument on `domains` / `contracts` is a QUERY, not an identifier: type
names are not unique, so a title matching several entities returns them all,
each tagged with its own id. Pass the id to get exactly one.

**Test / external symbols** — `--include-tests` and `--include-external` are
global flags, **default off** (focused output). Pass `--include-tests` (bare =
true) or `--include-tests=true|false` to include/exclude test-file symbols;
same for `--include-external` (stdlib / third-party stubs like Rust's
`Result::unwrap`). External rows are minimal stubs — no signature, source, or
docs; `kind` is best-effort from the name.

**Stylesheets (CSS/Sass)** — `.css`/`.scss`/`.sass` are indexed alongside code:
every class, id, and custom property is a searchable node; each stylesheet is a
module. Classes referenced from code (`className="btn"`, in files under
`[language.css] usage_sources`) link to their definition; `@use`/`@import` are
import edges; `@extend`/`composes` are extends edges.
- `kenn check css` — dead-CSS report: classes nothing uses (`orphan_class`) and
  stylesheets nothing imports (`orphan_stylesheet`). `orphan_class` needs
  `usage_sources` configured (else skipped with a note).
- `kenn check links` — non-exact markdown links (drifted/fuzzy/ambiguous/dangling).

**Read code**
- `kenn get source <id>` — the symbol's full source (whole item: doc comment /
  attributes through the closing brace). Rust needs rust-analyzer ≥ Dec-2025;
  older toolchains return just the declaration line.

**Findings store** (see below)
- `kenn findings search <query>`, `kenn findings get <id>`,
  `kenn findings add <text>`, `kenn findings merge <ids…> --text <t>`,
  `kenn findings predecessors <id>`, `kenn findings successors <id>`.

**Directives — code-anchored steering**
- `kenn findings directives <paths…>` — directives/guides relevant to the given
  changed paths, ranked by anchor match + liveness. Each hit is flagged `stale`
  (a cited code node vanished) or `drifted` (an anchored file changed since it
  was written) — re-read a flagged directive before relying on it.
- `kenn check findings` — findings whose anchors rotted: `broken` (path no
  longer resolves) and `drifted` (file changed content since attach).
- `kenn findings touch <fnd_id> --op attach|detach|rename` — append to a
  finding's anchor log. `attach` re-stamps the file's content sha (clears
  drift). `kenn findings add` also takes `--anchor` to create + anchor at once.

## Choosing a command

- Know the name → `kenn find symbol`. Fuzzy or by description →
  `kenn find symbols` or bare `kenn find <query>` (semantic).
- Orienting an unfamiliar repo → `kenn overview`, then a semantic `kenn find`.
- "Who uses / calls / implements X" → the `kenn list …` commands on X's id, or
  `kenn find usages <name>` in one shot.
- Reading code → `kenn get source`.

## The findings store — shared, durable memory

The findings store carries conclusions across tasks and sessions. Two habits
make it pay off:

**1. Search before you re-investigate.** Before digging into a question, run
`kenn findings search <query>` (or a semantic `kenn find <query> --scope both`)
— a prior conclusion may already be recorded. Each hit carries a `stale` flag:
when true, the finding's code evidence changed since it was written — verify
before relying on it.

**2. Store at a stable conclusion.** When you reach a verified fact, decision,
plan, or non-obvious gotcha, run `kenn findings add`. Store the durable result,
not every intermediate thought.
- `<text>` — the conclusion, stated plainly enough to be useful months later,
  out of this conversation's context.
- `--parent <id>` (repeatable) — the evidence: code-node ids (`<lang>:<pub_id>`)
  and/or earlier finding ids (`fnd_…`) it derives from. Provenance is what lets
  a later reader ask "why?" and `kenn findings predecessors` answer.
- `--tag <t>` (repeatable) — free strings, no enforced vocabulary. A useful
  starter set: `evidence`, `gotcha`, `plan`, `decision`.

**Corrections and retractions.** Findings are append-only — never edited in
place. To correct one, `kenn findings add` the corrected version with
`--tag supersedes:<old_id>` and `--parent <old_id>`; the old finding drops out
of search but stays readable via `kenn findings get`. To retract, add a
tombstone with `--tag tombstone:<target_id>`. To combine several into a
higher-level conclusion, `kenn findings merge <ids…> --text <t>` — it keeps the
inputs as parents.

## Directives — steering anchored to code

Beyond conclusions *about* the code, kenn stores **directives** (findings tagged
`directive` with a `polarity:do`/`polarity:dont` rule) and **guides** (tagged
`guide`, orientation/context), each *anchored* to the files/dirs it applies to
so it resurfaces where it matters — team-shared, agent-agnostic.

Three moments, three skills:
- **Starting work on an area** → the `recall` skill: pulls the directives/guides
  for the files you're about to touch (`kenn findings directives`), so you start
  informed (heed `polarity:dont` rules; treat guides as context; re-read any
  `stale`/`drifted` directive first).
- **Before a commit** → the `squeeze` skill: `kenn check findings` (repair moved
  anchors) → pull `kenn findings directives` for the staged diff and warn on
  violations → re-attach what applied → distill this session's
  directions/corrections into new directives anchored to the changed files.
- **Tending the store** → the `reconcile` skill: sweep the `broken`/`drifted`/
  `stale` signals, re-read each cited file, and refresh / supersede / detach /
  tombstone the rotted findings.

Two more, graph-aware:
- **Before changing a symbol** → the `blast` skill: walk the call/type/usage
  graph to the transitive **change surface** (callers ∪ usages ∪ implementers)
  and fuse `kenn findings directives` over the touched files — *what will this
  touch* ∪ *what governs it*, in one shot.
- **Understanding how a flow works** → the `trace` skill: walk the graph
  directionally (callees downstream / callers upstream / usages for data),
  re-read the key hops, and synthesize one narrative — optionally saved as a
  `guide` finding.

And, advisor:
- **Hunting duplication** → the `dup` skill: sweep `kenn find similar` over a
  scope to surface near-duplicate implementations, re-read each to confirm, and
  present consolidation candidates.
- **Auditing an area** → the `audit` skill: the deep pipeline — duplication
  (`dup`), dead code (empty `kenn find usages`), and god-modules
  (`kenn list callers` fan-in) — each candidate vetted against source, ranked
  into a plan, with "considered and rejected" stored as durable `reject`
  findings so the next audit doesn't re-flag them.

## Pagination & output

- Most `list`/`find` commands page. Pass `--all` to drain every page in one
  call; or `--page-size <n>` and `--cursor <tok>` (a prior response's `next`) to
  page manually. `next: null` means exhausted.
- Iteration commands (`list *`, `find usages`, `find similar`) walk the whole
  corpus with `--all`. Top-K commands (`find symbols`, bare `find`,
  `findings search`) return the top ~30 ranked results — past that, scores are
  noise, so refine the query rather than paging further.
- Default output is TOON; add `--json` and pipe to `jq` for scripting. A
  `ListResponse` renders as a header-once table with a trailing `next:` line.
