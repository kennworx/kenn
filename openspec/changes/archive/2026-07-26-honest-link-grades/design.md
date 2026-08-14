## Context

Markdown and HTML link resolution runs a recall-first ladder: `exact` →
`drifted` → `fuzzy` → `ambiguous` → `dangling`, never dropping a link. md↔md
resolution goes through `ResolutionIndex` (markdown documents only); anything
that misses falls through to md→code resolution against the store's `files` and
symbol tables. What misses *that* becomes a stub.

**Three copies of one join rule.** "Join a link-relative path onto the linking
file's directory" was implemented three times:

| implementation | location | `..` handling | above-root guard | drives |
|---|---|---|---|---|
| `join_relative` | `markdown/resolve.rs` (private) | pops a segment | `None` | md↔md grading |
| `canonical_path` | `html/links/core.rs` (private) | pops a segment | silent `""` | HTML fragment lookup, asset stubs |
| `normalize` | `markdown/code_resolve.rs` | **deletes the token** | n/a | **md→code and HTML→file grading** |

Two were right. The wrong one sat on the shared grading path both other callers
funnel into. Inside `html/links/core.rs` the split was visible in adjacent
methods: `fragment()` computed the target through `canonical_path` to find
anchors, while `resolve()` handed the raw href to `resolve_file_ref` for
grading. One href, two answers.

**Half an attachment model.** The other fault is not a bug in a rule — it is a
rule that exists on one side only. HTML resolves a reference to a real file kenn
does not index:

```
AssetIndex::exists(canonical)          // trait; html/ingest.rs backs it with the filesystem
  ├── true  → attachment stub keyed by canonical path, grade Exact
  └── false → stub keyed by the written string,        grade Dangling
```

`html-index` calls this "reusing the markdown attachment model", but markdown
has only `Kind::Attachment`, chosen by MIME guess — no existence check, so every
such target is `Dangling` whether or not it is there. And HTML's gate
(`is_asset_ref`) requires a *non-indexed extension*, so extensionless names and
directories never reach `mint_asset` on either side.

**What the graph contains.** On this workspace the `files` table holds markdown
633, rust 369, csharp 22, typescript 15 — 1039 rows, no directories, no
`LICENSE-MIT`. So those targets can never resolve to a *file node*; the question
is only whether they resolve to an *attachment node*, which is exactly what the
HTML half already builds.

## Goals / Non-Goals

**Goals:**

- One relative-path join, used everywhere a link is graded.
- One answer to "the target exists but kenn does not index it", shared by
  markdown and HTML, rather than the two answers shipped today.
- `dangling` means *nothing in the workspace matches this target*.
- The missing spec scenarios exist, so the implementation cannot regress to
  "only the drift case was specified."

**Non-Goals:**

- A new `LinkGrade` variant. The earlier draft of this design added
  `Unindexed`; it was dropped once the shipped HTML behavior was found (D2).
- Changing the `check_links` default filter or its `total` semantics.
- Changing HTML's asset *grading* (an existing target is still `Exact`, keyed by
  canonical path) or its stub id scheme. Its **eligibility gate** does change:
  review found that keeping `is_asset_ref` would have left an excluded `.md`
  dangling in HTML while markdown resolved it — a fresh divergence — so the gate
  is gone and existence decides on both sides (D2).
- Indexing directories or licences as *code*. They become leaf attachment stubs,
  not file-table rows.
- Fuzzy resolution (still deferred).

## Decisions

### D1 — Promote `join_relative`; delete `normalize` and `canonical_path`

**Decision.** One `pub(crate)` helper in `crates/kenn-indexer/src/relpath.rs`,
reachable from both `markdown` and `html`. It keeps `resolve.rs`'s above-root
guard (`None`) and absorbs `canonical_path`'s root-relative (`/…`) branch.

**Why not fix `normalize` in place.** That would leave three copies and invite a
fourth. The atlas work established the pattern: when two surfaces must agree,
extract the rule rather than reimplement it.

**The exact rung takes either spelling.** md↔md `resolve_inline` tries the path
*as written* (already workspace-relative) and *then* the joined path, grading
both `Exact`. `resolve_file_ref` now mirrors that. Trying only the joined form
would have broken `[t](src/order.rs)` written from `docs/a.md` against a
workspace-relative `src/order.rs`; trying only the as-written form is the bug.

**Consequence on the too-loose direction.** From
`crates/kenn-indexer/src/markdown/README.md`, `../../x/mod.rs` normalized to
`x/mod.rs` and could grade `exact` against a root-level `x/mod.rs` — the wrong
file. It now joins to `crates/kenn-indexer/x/mod.rs`, matches nothing, and
correctly degrades. Fixing the exact rung fixes both directions at once; they
are the same comparison.

**A third, intended behavior change.** A bare sibling name resolves by the join
rather than by locality: from `api/docs.md`, `[t](order.rs)` *means*
`api/order.rs` and now grades `Exact` where it previously reached the locality
rung and graded `Drifted`. This is the same missing join, and the existing test
that asserted `Drifted` was asserting the bug. It is rewritten, and a case that
genuinely needs locality (a stale path no join can satisfy) keeps that rung
covered.

### D2 — No new grade: finish the attachment model instead

**Decision.** An unresolved markdown target that exists in the workspace becomes
an `attachment` stub keyed by its **canonical workspace-relative path**, graded
`Exact` — byte-identical in shape to what `mint_asset` already does for HTML.
Eligibility is decided by existence, not spelling. HTML's `is_asset_ref` gate —
"has a non-indexed extension" — is **deleted** rather than ported: `mint_asset`
already dangles a target the workspace does not hold, so the gate only ever
suppressed *existing* targets whose extension kenn indexes. Extensionless files
and directories are therefore in, and so is an excluded `.md`.

**What this replaces.** An earlier draft added `LinkGrade::Unindexed` plus a
default-filter change so the bare report would hide it. Finding the shipped HTML
behavior killed that design:

- It would have produced a *third* answer to one question — `logo.png` graded
  `Exact` while `LICENSE-MIT` graded `Unindexed`, for the same situation. That
  is the disease this change treats.
- `Exact` means "path and name current." For a file that is present at the path
  written, that is simply true. The link is not degraded; it resolves.
- Minting the node is strictly more useful than grading the absence: a
  path-keyed stub means two documents linking `LICENSE-MIT` reach the **same**
  node, so `list usages` on it works. `html-index` already states this is why
  path-keying exists ("what makes reverse lookup deterministic").

**Cost accepted.** `check_links` can no longer answer "what do my docs reference
that kenn does not index?" — those links are `Exact` and never appear in the
report. That question is answerable from the graph instead (attachment nodes and
their usages), and it is not the question `check_links` exists to answer.

**One kind rule, shared.** `existing_target_kind` decides what a resolved target
is: `Document` when the name is something navigable kenn would have indexed —
a `.md` or `.html` the config excluded — and `Attachment` otherwise, covering
binaries, extensionless files like `LICENSE-MIT`, and directories.
`Kind::Attachment` is documented as "a leaf stub node, not a navigable
document", which fits a directory: there is nothing to navigate into, and the
link is valid. Both corpora call this one function, so an on-disk target is
never a leaf on one side and a document on the other — the divergence review
caught when markdown hardcoded `Attachment`.

### D3 — Existence comes from the trait that already exists

**Decision.** Reuse the `AssetIndex`-shaped seam: the resolver takes an
existence oracle as an injected dependency and performs lookups only; the caller
decides what backs it. The trait moves to `relpath.rs` as `PathExists` so both
corpora share it, and the filesystem backing (`FsPaths`) moves with it —
review found the first cut had unified the trait but left two byte-identical
backings, one per module. The backing widens from `is_file()` to `exists()` so a
directory target resolves.

**Why not a walked-path set.** An earlier draft proposed retaining the paths
`markdown::discover` visits. That would have been a *second* oracle beside the
trait already in the tree, scoped to the markdown roots rather than the
workspace — so a workspace narrowing `roots` would silently narrow what counts
as "exists". The trait keeps the resolver pure while letting the caller choose,
which is the property that mattered.

**Scope caveat.** The oracle answers about the workspace root. A target outside
it — or one the caller's backing declines — stays `Dangling`. That is defensible
("not in the indexed workspace") and is pinned by a scenario.

### D4 — Order of attempts

For a target that misses md↔md resolution:

```
1. md→code file/symbol resolution (with the shared join)  → exact | drifted | ambiguous
2. exists in the workspace                                → attachment stub, exact
3. otherwise                                              → dangling stub, dangling
```

Existence is checked *after* graph resolution, so an indexed source file
resolves to its **file node** and never degrades to an attachment stub merely
because it is also on disk.

**One exception, added after review: a bare name on an inline link.** A target
that is neither path-shaped nor extension-bearing (`is_code_path` is false) goes
down the *symbol* branch, where any same-named symbol matches. So
`[the docs](docs)` in a README became a `links_to` edge to a `fn docs` rather
than the directory. An inline `CommonMark` destination denotes a path, so when
the workspace holds that path it wins. Two boundaries keep this narrow: a
**wikilink** is the opposite convention — a bare `[[OrderHandler]]` denotes a
name — and keeps symbol-first; and a **path-shaped** target still resolves
through the file branch to its indexed file node.

**The check applies to every unresolved target, path-shaped or not** — it is a
lookup of the written target (joined, when relative). A wikilink is a *name*
rather than a path, so `[[docs]]` resolves when a `docs` directory exists, while
`[[gone]]` stays `Dangling` because nothing in the workspace is at that path.
Special-casing name-shaped targets would buy a second rule about which targets
are "path enough" to check — another divergence to keep in sync.

### D5 — What "1, not 7" means as a gate

The success criterion is a whole-repo assertion, not a unit test, and belongs in
the verification tasks: after `kenn index --force`, `check_links` reports one
`dangling` row — the `[[feedback_no_version_bumps]]` wikilink in an archived
design doc, which names a machine-local memory file and is genuinely broken —
and `indexers/frames.ts` is graded `exact`. Unit tests cover the rungs; this
covers the conflation.

## Risks / Trade-offs

- **A test that passes for the wrong reason.** CLAUDE.md §9 has bitten this repo
  on exactly this shape. → Mutation-verify each guard individually and confirm
  it fails *for the stated reason*. Done for D1: restoring `normalize` turned
  all three relevant tests red with distinct, predicted messages, including the
  false-`exact` assertion. Repeat for the attachment guards.

- **Widening `is_asset_ref` could swallow genuine breakage.** An extensionless
  href that happens to collide with a real path now resolves instead of
  dangling. → Existence is the gate, so it only "swallows" links that point at
  something real. The whole-repo count in D5 is the backstop: if a real typo
  lands in the attachment bucket, the dangling count moves and the gate fails.

- **A route-shaped href in a web app** (`<a href="/about">`) resolves if a
  directory named `about` happens to exist. → Accepted: that is a correct
  statement about the workspace, and the alternative (guessing which
  extensionless hrefs are routes) is unspecifiable.

- **Re-grading requires a re-index.** Existing snapshots keep their old grades
  until `kenn index --force` — already the standing rule. No schema change at
  all under this design, so no migration and no version interaction.

## Migration Plan

None. No schema change, no new discriminant, no store-format interaction.
`kenn index --force` re-grades.

## Open Questions

- Should `check_links` eventually report attachment-resolved links as a distinct
  view ("what do my docs reference that kenn does not index?")? D2 accepts
  losing that from the report; the data is still in the graph. Revisit only if
  someone actually asks the question.
