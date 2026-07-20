## Context

`lance-search-backend` establishes a committed, git-merge-clean Lance store with a custom `CommitHandler`, immutable UUID/ULID-named files, index preservation across merge, and a pluggable embedding boundary. It indexes *code* — symbol/doc text and embeddings.

This change adds the *knowledge* layer on the same engine. A finding is a durable, agent-derived statement about the codebase — an invariant, a rationale, a gotcha — with provenance: which code (or which earlier findings) it was derived from. The design problem is the record shape, the provenance model, how findings stay consistent as code moves, and how they reach the committed store.

The earlier exploration settled most of this; this document records the decisions and the few open points.

## Goals / Non-Goals

**Goals:**
- A durable finding record, committed and merge-clean, reusing the `lance-search` storage guarantees.
- Provenance: every finding traces to the evidence it was built from, including other findings.
- Hybrid search over findings; one engine shared with code search.
- Findings stay correct as code changes — without mutating stored findings.
- A simple, explicit path from "agent stored a finding" to "it is in the committed store."

**Non-Goals:**
- The MCP tool surface and the subagent-as-extractor dispatch pattern — `findings-mcp-layer`.
- Embedding-model selection — inherited as out-of-scope from `lance-search-backend`.
- Automatic semantic de-duplication of near-identical findings — surfaced to the caller, not resolved here.
- Review-before-flush UX — a later refinement.

## Decisions

### D1 — Findings are rows in the Lance store, not a separate engine

A finding is a row: `{ id, text, embedding, tags[], parent_ids[], created_at }`. It reuses the `lance-search` dataset (or a sibling Lance dataset with the same `CommitHandler` and merge model). One engine means one hybrid-search path over code *and* knowledge, and zero new merge machinery. Alternative considered — a separate relational/KV store for findings — was rejected: it would fragment search and re-introduce a merge problem already solved.

### D2 — Free-form `text` + `tags`, no `kind` enum

The finding payload is prose `text` plus a list of free-form `tags`. A fixed `kind` enum was considered and rejected: different task types (fix, feature, comprehension) and different orchestrators want different vocabularies, and an enum calcifies. Tags are searchable, multi-valued, and convention-driven — the vocabulary evolves without a schema migration.

### D3 — Unified ID space; provenance is a DAG

`parent_ids` reference code-graph nodes *or* other findings — both inhabit one ID space. A finding's parents are the evidence it was built from; `merge_findings` records its inputs as parents. The result is a derivation DAG: `find_predecessors` / `find_successors` walk it. This makes "where did this conclusion come from?" structurally answerable, down to source code. The DAG is acyclic by construction — a finding can only parent earlier-created findings.

### D4 — Append-only; supersede and tombstone instead of mutate

Findings are immutable once written. This is what keeps the store merge-clean (the `lance-search-backend` guarantee depends on write-once, uniquely-named files).

- **Correction / refinement:** a new finding with the old one in `parent_ids` and a `supersedes:<id>` tag. History is preserved; retrieval prefers the latest in a supersede chain.
- **Deletion:** a tombstone finding referencing the target; retrieval filters tombstoned ids.
- **No in-place edits, ever.**

### D5 — Staleness is computed at read time, never stored

A finding references code-graph evidence through `parent_ids`. When that code changes or disappears, the finding may no longer hold. Storing a `stale` flag would mutate the finding (violating D4) and would be branch-incorrect (a finding stale on `main` may be live on a branch where the code still exists).

Instead: at query time, the store checks whether a finding's evidence node ids still resolve in the *current branch's* code graph; if not, the result is flagged stale. Staleness becomes a pure function of `(finding, current code graph)` — immutable findings, branch-correct staleness. A stale finding is still returned (it is useful as "this used to be true"), just marked.

### D6 — Pending → canonical flush lifecycle

`store_finding` writes to a pending area (the local Lance write buffer / a pending dataset region). Findings move into the committed store on an explicit **flush**. Default policy: flush every pending finding the caller did not explicitly drop. Batching avoids one git-noisy commit per finding; the explicit flush keeps "what gets committed" a deliberate act. Review-before-flush is deferred (Non-Goal).

## Risks / Trade-offs

- **Near-duplicate findings** — two tasks independently record the same fact. → Mitigation: `store_finding` runs a similarity check and returns near-matches to the caller, which decides (supersede, link, or accept). The store does not auto-merge — that is a judgement call left to the agent layer.
- **Supersede-chain retrieval cost** — following `supersedes` chains on every read. → Mitigation: chains are short; resolve lazily and cache within a query.
- **Staleness check cost** — verifying evidence ids per result. → Mitigation: batch the existence check against the code graph; it is a set-membership test, cheap.
- **Embedding cost for findings** — each finding needs an embedding for semantic search. → Mitigation: same pluggable producer and batching as `lance-search-backend`; findings are far fewer than code symbols.

## Migration Plan

- Additive: findings are new rows in an existing committed store. No existing data migrates.
- The Lance layout note from `lance-search-backend` covers the added row variant; no separate snapshot break.
- Rollback: revert the change. Findings rows become unread by the reverted code; the code-search rows are unaffected. Committed findings are the only at-risk data — and they are recoverable from git history.

## Open Questions

- **Exact finding schema.** Beyond `{ id, text, embedding, tags[], parent_ids[], created_at }` — does a finding need an explicit `task` field, a `produced_by` field, or are those just tags? Lean: tags, to keep the schema minimal.
- **`merge_findings` semantics.** Does it produce a new synthesized finding (inputs as parents, originals kept) or supersede the inputs? Lean: synthesize-and-keep — the originals remain as evidence.
- **One dataset or two.** Code rows and finding rows in one Lance dataset (simplest hybrid query) versus sibling datasets sharing the `CommitHandler` (cleaner separation of throwaway-rebuilt code rows from durable findings). Lean: sibling datasets — findings are durable, code rows are reconciled-on-rebuild, and mixing the two lifecycles in one dataset complicates reconciliation.
- **Pending area representation.** A separate pending Lance region, or an in-memory buffer flushed in one commit. Decide during implementation.
