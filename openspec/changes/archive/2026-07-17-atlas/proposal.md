## Why

Kenn's index is invisible to the agents it exists to serve. The structural
knowledge — packages, the call graph, communities, central symbols — lives in
SQLite behind MCP/CLI tools that an agent only consults *if it thinks to ask*.
But at cold-start in an unfamiliar (or freshly-cloned) repo, an agent doesn't
yet know what's there, so it falls back to the passive signals it's trained to
inhale: `README`, `CLAUDE.md`, a few globs. The one channel agents can't ignore
— markdown they read up front — is exactly the channel kenn never writes to.

Kenn already *computes* the map (and even renders it as `kenn visualize` HTML for
humans). This change serializes that same projection for the other reader in the
room — the agent — in its native format: plain markdown it reads to orient before
working.

## What Changes

- **`kenn index` produces an "atlas"**: an [Open Knowledge Format (OKF v0.1)](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
  bundle of plain-markdown concept docs + reserved `index.md` + `log.md`, written
  at a kenn-owned (`Layout`-resolved) location that works for the local repo, a
  foreign workspace (`kenn index -d ./repo`), a worktree, or a custom store.
- **kenn is a purely *structural* producer.** It emits only checkable facts —
  packages, each package's most central symbols (its own top-degree members),
  directed dependency edges, member files, root module docs verbatim — into
  concept skeletons. It never writes semantic prose ("what this is for"), because
  it cannot reason. Its facts ride in producer-defined `kenn.*` frontmatter keys
  (a property that matters once cross-index persistence lands; v1 regenerates the
  bundle wholesale each index).
- **The agent enriches, and the trust boundary equals the reasoning boundary.**
  Kenn's facts are ground truth by construction; only agent prose can be wrong. In
  v1 the agent enriches its *understanding* **in-context** to orient — kenn does
  not persist agent prose (cross-index persistence is a named follow-on).
- **A markdown handle, not JSON, and no hardcoded paths.** `kenn index` announces
  the atlas with a marked, greppable line naming its `index.md` (and, under the
  existing `--json` mode, a field on the completion event — never a bare line in
  the JSON stream). That file *is* the handle — a re-readable map + shape/status
  header the agent rereads to re-orient.
- **Consumption is a skill, not MCP**: a drop-in `skills/atlas/SKILL.md` in kenn's
  existing plugin whose steps are path-free (run index → read the printed file →
  enrich skeletons → work), with a trigger-rich `description` for passive
  discoverability. **No new MCP surface.**
- **v1 scope**: one concept per **internal (non-external) package** (external deps
  excluded; manifest-less code deferred); skeleton bodies; `description` seeded
  verbatim from the root module doc. Enrichment is in-context per session (not
  persisted in v1).

## Capabilities

### New Capabilities
- `atlas-bundle`: `kenn index` emits an OKF-conformant markdown bundle derived
  from the code graph (concept-per-package skeletons, `index.md`, `log.md`) at a
  `Layout`-resolved location, and prints a markdown handle naming its `index.md`.
  Covers the concept `type` taxonomy, the `kenn.*` frontmatter contract, and the
  path-free, markdown-first consumption contract the `skills/atlas` skill relies on.

### Modified Capabilities
<!-- No existing spec's REQUIREMENTS change: the atlas reads graph-analysis /
     code-intel-data-model / store-layout as-is, and hooks into indexing as a new
     post-aggregate output rather than altering index-run-reporting's contract.
     These touchpoints are listed under Impact. -->

## Impact

- **Builds on (reads, unchanged):** `code-intel-data-model` / `source-data-model`
  (symbols, packages, `file_docs` module docs, and the **raw directed `edges`** —
  the source for directed package dependencies), `graph-analysis` `aggregate_nodes`
  + `weighted_degree` (anchor = package/module rollup + per-anchor centrality),
  `store-layout` (bundle location). Note: `aggregate_edges` is undirected and the
  god-node list is global, so the producer derives directed deps + per-package
  centrality itself rather than reusing those rollups (see design D7).
- **Hooks into:** a shared `finalize_atlas` step in `indexing-orchestrator`, called
  by both `cmd_index::run_async` (CLI) and `workflow::index_workspace` (MCP) after
  the run's code graph is persisted (reads the run's `code.db` via the Reader API,
  independent of the optional analysis pass) — and `index-run-reporting` (the marked
  handle line + `--json` field).
- **New surface:** the `.../atlas/` OKF bundle on disk; a `skills/atlas/SKILL.md`
  in `claude-plugins/kenn`; no new MCP tools, no new required config.
- **Prior art / differentiation:** [understory](https://github.com/thecodacus/understory)
  independently converged on the same "conformance in code, not prompts" boundary
  and a session-start seed for discoverability — but it is *generic LLM-authored
  memory with no code understanding* (it even defers the FTS5+embedding search
  kenn already ships). Kenn atlas is the **code-derived** counterpart: the bundle
  comes from kenn's real call graph — the thing understory structurally cannot
  produce. That is kenn's moat.
