> **Sequencing.** Land section 1 (md format) and verify green before sections
> 2–5 — it touches the derived-store rebuild parser and every reader and is
> independent of directives.

## 1. Storage: md record format (findings-store, store-layout)

- [x] 1.1 Change the finding record from `<id>.json` to `<id>.md` with YAML
      frontmatter (`id`, `tags`, `parent_ids`, `created_at`) + prose body. No
      migration — the one prototype record was dropped and the shape changed in
      place (`serde_yaml_ng` for the frontmatter).
- [x] 1.2 Make the embedding source the md **body** (prose), not the frontmatter.
      The body == the in-memory `Finding.text`, so the existing text-fingerprint
      keying and committed embeddings reuse with no re-embed (verified by the
      `flushed_finding_retrieved_by_paraphrase` integration test).
- [x] 1.3 Route `findings/<id>.md` through `Layout` accessors; keep it committed
      (records under `findings_dir()`, tracked; `.tmp/` staging gitignored).

## 2. Anchors + liveness sidecar (findings-store, store-layout)

- [x] 2.1 Added the per-finding `<id>.anchor.jsonl` append-only event log
      (`attach` / `rename` / `detach`, `ts` only — no commit hash) in
      `db/findings/anchor.rs`; `fold` → current anchor set with recency (latest
      `attach` ts) + attach_count. The recency-*weighting* (decay) is applied at
      retrieval (§4) with the current clock, never auto-retiring — the fold stays
      deterministic/clock-free. Exposed via `FindingsStore::record_anchor_event`
      / `anchors_for`.
- [x] 2.2 Anchor logs live under `findings_dir()` (Layout-routed, committed);
      `.anchor.jsonl` is excluded from record reads (only `.md` are records).
- [x] 2.3 Blessed via `lifecycle::is_directive_or_guide` (+ `TAG_DIRECTIVE` /
      `TAG_GUIDE` consts), consumed by `find_directives` — no new record kind.
      `polarity:*` is read by the agent-side guardrail (skill/fragment), so the
      store needs no polarity logic.
- [x] 2.4 On supersede, seed the successor's anchor log with the predecessor's
      current anchor set so a correction stays reachable by `find_directives`.

## 3. Orientation snapshot (store-layout)

- [x] 3.1 `kenn index` writes a run-local `overview.md` into the active run dir
      (`persist_run_artifacts`, runs-centric, no snapshots dir). **v1 deviation:**
      content is rendered from `SnapshotMeta` (counts + `indexed_at` + status +
      failed_projects) — languages/packages are omitted (they'd need a mid-index
      DB query) and it does not yet share a generator with `get_workspace_overview`
      (cross-crate). Both are follow-ups, noted in design.
- [x] 3.2 Confirm "file absent" is the readiness signal (no resource, no error
      channel); the guide names the fallback tool and uses `indexed_at` to detect
      a stale-but-present snapshot.

## 4. MCP tools (findings-mcp)

- [x] 4.1 `find_directives(paths)` — RRF of anchor exact-path + ancestor-dir
      match ⊕ body-vector proximity, boosted by liveness; filtered to
      `tag:directive` / `tag:guide`. Reuse the kenn-store identifier-unified
      fusion. Degrade to the anchor leg alone when the embedder/index is cold
      (`-32002` family) instead of erroring. Respect supersede/tombstone — prefer
      the latest in a chain, exclude tombstoned.
- [x] 4.2 `check_anchors` — fold every `<id>.anchor.jsonl`, test each file/dir
      anchor against the filesystem (no index needed in v1), report unresolved
      ones (optionally annotate with a git-detected rename candidate — design
      decision).
- [x] 4.3 Anchor-event recording (`attach` / `rename` / `detach`) — tool vs.
      direct file-append (design decision); keep the liveness rollup consistent.
- [x] 4.4 Add an optional `anchors` list to `store_finding` — record an initial
      `attach` per anchor so a directive is created and anchored in one call.
- [x] 4.5 Extend the installable, orchestrator-independent system-prompt fragment
      to drive the directive workflow (recall by file/dir before work; the
      before-commit check → pull → re-attach → squeeze ritual; routing + redaction
      rules). The fragment is the single source of truth; the plugin skills
      (§5) surface it for Claude Code.

## 5. Agent-guide layer (agent-guide — plugin)

- [x] 5.1 Refocus the kenn plugin guide into a router: tools (navigate),
      snapshot (orient), skills (squeeze / recall), and the before-commit ritual.
      Add the missing-file fallback instructions and negative triggers.
- [x] 5.2 `recall` skill — pull directives/guides relevant to the file(s)/dir(s)
      under work via `find_directives`; never fabricate (if empty, say so).
- [x] 5.3 `squeeze` skill — the before-commit ritual: `check_anchors` → pull by
      file/dir (judge relevance, guardrail) → re-attach the relevant ones →
      distill new directives anchored to changed files/dirs (supersede if
      changed), applying the constitution / case-law / personal-note routing rule
      and asking the user when unclear. Apply a redaction gate before writing a
      committed directive — no credentials, machine-local paths, or private
      project/customer identifiers; route unshareable content to a personal note.
- [x] 5.4 Source the squeeze's directions + session touched-files from the
      existing `conversation-history-store` (`collector.db`, branch-filtered) and
      `transcript_path` — no new capture mechanism; it is a read dependency.

## 6. Verification

Covered by automated tests (workspace clippy 0, CRAP gate passed, fmt clean,
720+ tests green):

- [x] 6.1 A finding round-trips as md (frontmatter + body); the body is the embed
      source; `parent_ids` immutable. (`record.rs` tests; body-with-`---`
      round-trips.) Migration was dropped per decision — N/A.
- [x] 6.2 Anchors fold across `attach` / `rename` / `detach`; a repeat `attach`
      bumps recency + count; `rename` carries liveness; `detach` removes.
      (`anchor.rs` tests.) No-md-churn / merge-clean hold by construction
      (per-finding append-only files).
- [x] 6.3 `find_directives` returns directives anchored to a file and to an
      ancestor dir, liveness-ordered; empty when nothing matches; semantic-only
      matches surface. (`directives.rs` tests + the `kenn-mcp` flow test.)
- [x] 6.4 `check_anchors` reports an anchor whose path doesn't resolve.
      (`kenn-mcp` flow test.)
- [x] 6.5 `store_finding` with `anchors` creates + anchors in one call.
      (`kenn-mcp` flow test.) `record_anchor` attach/rename + unknown-op error
      also covered there.
- [x] 6.9 A superseding directive inherits the predecessor's anchors.
      (`anchor.rs` `supersede_seeds_successor_from_predecessor`.)

Implemented and exercised indirectly, but without a dedicated assertion
(follow-up tests):

- [x] 6.10 `stale` flag — `rank_directives` sets it via `finding_is_stale`
      (same path as `search_findings`); cold-index degrade and supersede/tombstone
      exclusion are implemented in `find_directives` (mirroring the tested
      `search_findings`) but not separately asserted.
- [x] 6.11 Anchor events at create flush with the finding (flow test). The
      derived-store rebuild reads `<id>.md` and the `.kenn/vectors/findings/`
      sidecar (`record.rs` + `hybrid_search.rs`). Drop-discards-orphan path is
      not separately asserted.
- [x] 6.7 `overview.md` is written by `persist_run_artifacts` with `indexed_at`;
      the guide/fragment spell out absent→tool and stale→freshness-check. The
      CLI render is not unit-tested yet.

Agent/skill behavior — not unit-testable code (verified by reading the
skill/fragment text, exercised at runtime):

- [x] 6.6 The before-commit ritual guardrail (warn on `polarity:dont` violation,
      then capture) — encoded in the `squeeze` skill + the system-prompt fragment.
- [x] 6.8 The redaction gate (no secrets/private ids in committed directives) —
      encoded in the `squeeze` skill + the fragment.
