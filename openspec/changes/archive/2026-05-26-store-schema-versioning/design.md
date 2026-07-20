## Context

`kenn-store` resolves every path through a single `Layout` and publishes snapshots into `.kenn/runs/<id>/` (per the `store-layout` capability). A reader picks the snapshot via a content-based staleness key (`workspace-staleness`: git HEAD + dirty hashes) — that key answers "did the source change?" but not "is this snapshot shaped the way my current binary expects?".

The two are orthogonal failure modes. Today only the first is detected. `fix-symbol-def-ranges` proved the cost: a schema-semantic change (line basing) shipped, but old `.kenn/` dirs kept being opened, and `get_source` silently returned wrong content with no signal.

## Goals / Non-Goals

**Goals:**

- A binary that opens a snapshot it does not understand fails fast with an actionable message instead of serving wrong data.
- The bump-the-version step is cheap (one constant + one changelog line) so future schema fixes can include it without friction.
- The MCP server self-heals from schema-mismatch the same way it self-heals from other `Failed` states.

**Non-Goals:**

- Major.minor versioning. We're a single-user, single-binary tool today; the asymmetry "old binary reads new snapshot" doesn't have a real user. A single integer with strict equality is sufficient. If/when a genuinely additive change appears with a concrete forward-compat motivation, this design can be extended; until then, YAGNI.
- Automatic schema migration (rewriting v1 snapshots into v2 in place). The schema changes we've shipped so far all required re-deriving from source. A migration framework would be machinery for a use case that hasn't appeared.
- Tooling that enforces "every schema-affecting PR bumps the constant." Honor system + code review. A test that fails loudly when `STORE_SCHEMA_VERSION` is bumped without a corresponding changelog entry is the most we'd want — and even that is overkill for a two-developer rhythm.

## Decisions

### Decision 1: Single `u32` constant, strict equality

**What:** `pub const STORE_SCHEMA_VERSION: u32 = 2;` in `kenn-store::lib`. Persisted in each snapshot. Opened readers compare with `==`; anything else is a mismatch.

**Why:** A single number is the smallest model that answers the question we actually have: "is this snapshot one this binary knows how to read?". `==` keeps the policy unambiguous. Strings ("2.0", "2.1") and tuples invite negotiation logic ("can v2.1 read v2.0?") that, in a single-user tool, has no concrete user — and every line of negotiation logic is a place where the policy can drift from intent.

This aligns with [[feedback_no_version_bumps]] — *don't build versioning machinery for hypothetical users* — by minimizing the machinery and tying its existence to a concrete already-shipped failure mode.

**Alternatives considered:**

- Major.minor `(u16, u16)`. Lets a v2.0 reader trust a v2.5 snapshot. Rejected: no current user has multiple kenn versions opening the same snapshot; the additive-schema case (the only thing minor would unlock) has not arisen yet. Promote later if a real example appears.
- Hash of the schema definition. Forces drift detection on every code change. Rejected: massively over-triggered — a renamed local variable in `kenn-store` would bump the hash; the false-positive rate makes the signal meaningless.

### Decision 2: Persist in the snapshot's existing metadata

**What:** Write `schema_version` into the existing `meta.json` (or equivalent) that every published snapshot already carries. Read it on open via the existing metadata-loading path; no new file format, no new I/O surface.

**Why:** Snapshots already have a metadata blob (`indexed_at`, snapshot id, etc.). Adding one field is one struct addition + one serde line. A separate `SCHEMA_VERSION` file would mean a second I/O syscall on every open and a second possible corruption mode ("schema file exists but is malformed").

**Edge cases:**

- Snapshots written before this change have no `schema_version` field. On read, default to `1`. Combined with the constant being `2`, every existing snapshot resolves to mismatch — which is exactly what we want.
- Concurrent reader/writer is already serialized by the existing snapshot-publish protocol (atomic `live` symlink swap); the new field doesn't change that.

### Decision 3: Mismatch routes through the existing `Failed` lifecycle state

**What:** When `kenn-store` returns a `SchemaMismatch` error on snapshot open, the lifecycle layer (`kenn-mcp::indexing`) maps it to `LifecycleState::Failed { error: "schema vN, binary expects vM; reindex required (see SCHEMA_CHANGELOG.md)" }`. Under `kenn mcp`, the existing `spawn_recovery_pipeline` machinery (already covering Failed→Indexing transitions for other reasons) kicks in automatically.

**Why:** The Failed state already exists, already has a recovery path, and is already wired into `get_index_status` and the `reindex` tool. Adding a new lifecycle state for "schema-stale" would mean teaching every consumer about it; reusing `Failed` makes the new behavior fall out of existing code paths for free.

**Trade-off:** The `Failed` state semantically means "the indexer crashed or failed to produce output." Conflating with "the snapshot is from an incompatible version" loses some categorical clarity. The cost is small — the `error` string is structured enough to distinguish, and tests that care can match on the message prefix. The benefit (one path instead of two) is large enough to justify the conflation.

### Decision 4: CLI is explicit, MCP is auto-recovering

**What:**

- `kenn mcp` schema-mismatch open → automatic recovery reindex (free via the existing Failed→Indexing pipeline path).
- `kenn status`, `kenn search`, etc. → print the mismatch error, exit non-zero. The user runs `kenn index` explicitly.

**Why:** Under MCP the recovery loop is part of the contract (agents expect the server to converge to Ready). Under the CLI a silent multi-minute reindex would surprise the user — the explicit `kenn index` puts the time cost where the user can see it.

## Risks / Trade-offs

- **Discipline.** Bumping the constant requires manually adding a changelog entry. If someone forgets, the binary still works (the snapshot just gets rejected with the old version's error message). Worst case is mild confusion, not corruption. Acceptable.
- **Spec divergence.** A schema-affecting change that *doesn't* bump the constant ships corrupt-snapshot risk for the next release. Mitigation: every change proposal that modifies `code-intel-data-model`, `store-layout`, or `source-data-model` should list "bump `STORE_SCHEMA_VERSION`?" in the Impact section, the same way they already list test coverage.
- **Changelog rot.** If we ever ship many minor refactors without bumping, the changelog stays trustworthy. If we ship many semantic changes without bumping, the changelog is a fiction. The honor system holds it together.

## Migration Plan

1. Land the constant at `v2`. Every existing snapshot is implicitly v1, fails the check on first open, triggers a reindex (MCP) or an error (CLI).
2. No data migration. Same blast radius as `fix-symbol-def-ranges`: the user had to reindex anyway.
3. Add a one-line note in the next release's CHANGELOG.md (the user-facing one, if any) so reindexing-after-upgrade is signposted.

## Open Questions

- Should the constant live in `kenn-store::lib` or a dedicated `kenn-store::schema_version` module? Doesn't matter much — propose `lib.rs` for discoverability.
- Should `kenn status` distinguish "schema-stale" from "indexer crashed" in its output, even though the underlying lifecycle state is the same? Probably yes — the action the user takes is different (reindex vs. file a bug). Cheap to do via error-string sniffing.
