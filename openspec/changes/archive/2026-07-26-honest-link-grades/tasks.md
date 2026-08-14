## 1. Share the relative-path join (D1)

- [x] 1.1 Promote `join_relative` into `crates/kenn-indexer/src/relpath.rs` as a
  `pub(crate)` helper both `markdown` and `html` reach, keeping its above-root
  `None` guard and absorbing `html::links::core::canonical_path`'s
  root-relative (`/…`) branch.
- [x] 1.2 Use the shared join in `code_resolve::resolve_file_ref` — as written
  **or** joined, both `exact`, mirroring md↔md `resolve_inline` — and delete
  `code_resolve::normalize`. Leave the basename + locality `drifted` rung
  unchanged.
- [x] 1.3 Replace `html::links::core::canonical_path` call sites with the shared
  helper, including the above-root case now yielding unresolved rather than
  silently resolving against the root.
- [x] 1.4 Unit-test the exact rung: `../frames.ts` from
  `indexers/kenn-dotnet/README.md` grades `exact`.
- [x] 1.5 Unit-test the too-loose direction with a mocked `CodeLookup`: two
  same-basename files at different depths, a link walking up past one — assert
  it is NOT graded `exact` against the wrong file.
- [x] 1.6 **Mutation-verify.** Restoring `normalize` turned all three affected
  tests red with distinct, predicted messages — including
  `left: Exact, right: Exact` on the false-exact assertion. Restored, 509 green.
- [x] 1.7 Record the third behavior change D1 caused: a bare sibling name
  (`[t](order.rs)` from `api/docs.md`) now grades `exact` via the join instead of
  `drifted` via locality. The existing test asserting `drifted` was asserting the
  bug; rewrite it and keep a genuine locality case (a stale path no join can
  satisfy) covering that rung.

## 2. Finish the attachment model on the markdown side (D2, D3, D4)

- [x] 2.1 Give markdown an injected existence lookup, shaped like the existing
  `AssetIndex` seam, so the resolver performs lookups only and the caller owns
  the filesystem. Do not build a second walked-path oracle.
- [x] 2.2 In `markdown::ingest::core`, apply the D4 order: after md→code
  resolution misses, check existence of the canonical path (written target
  joined onto the linking file's directory). Exists → `attachment` stub keyed by
  that canonical path, grade `exact`. Missing → today's written-string stub,
  grade `dangling`.
- [x] 2.3 Key the markdown attachment stub the same way HTML keys its asset stub
  so one on-disk target reached from both corpora collapses to a single node.
- [x] 2.4 Widen `html::links::core::is_asset_ref`: an extensionless name and a
  directory-shaped path are eligible; existence decides, not spelling.
  *(Superseded by 5.11 — the gate was deleted outright once review showed it
  still dangled an existing-but-excluded `.md` that markdown resolved.)*
- [x] 2.5 Widen the `AssetIndex` filesystem backing in `html/ingest.rs` from
  `is_file()` to `exists()` so a directory target resolves. *(The trait is now
  `relpath::PathExists` and the backing moved there too — see 5.7.)*
- [x] 2.6 Unit-test the markdown cases: extensionless file that exists →
  attachment + `exact`; directory that exists → attachment + `exact`; target
  that does not exist → `dangling`; two spellings → one node.
- [x] 2.7 Unit-test that an indexed source file still resolves to its **file
  node** and is not shadowed by an attachment stub (D4 ordering).
- [x] 2.8 Unit-test the HTML cases: extensionless href that exists → attachment;
  the same href when absent → dangling.
- [x] 2.9 **Mutation-verify 2.6–2.8 individually.** Oracle forced `false` → both
  attachment tests red. Oracle ignored → the dangling test red, plus
  `end_to_end_corpus_graph`. Inverting the D4 order initially **survived**: the
  md→code fixture registered `src/order.rs` in the store without creating it on
  disk, so the existence rung was unreachable and the ordering unguarded. Making
  the fixture realistic exposed a real defect in this change — `attachment_key`
  tried only the *joined* spelling while `resolve_file_ref` accepts as-written
  **or** joined, so the two rungs disagreed about what a written target means.
  Fixed; the inverted-order mutation now drops the `links_to_file` backlink to 0.
- [x] 2.10 Mutation-verify the HTML widening: restoring the narrow
  `is_asset_ref` gate (`None => false`) turned both attachment tests red, and
  the "still dangles" test correctly stayed green (it guards against
  over-resolving, not under-resolving). Also found: `stub_kind` guesses
  Attachment-vs-Document by extension, so a confirmed-existing extensionless
  target was recorded as a `Document`. Added `attachment_symbol`, mirroring
  markdown's `attachment_stub` — once existence is confirmed there is nothing to
  guess.

## 3. Verify on the real workspace (D5)

- [x] 3.1 `just build-cli` then `kenn index --force` on this workspace, and
  **re-derive the pre-fix baseline from that run** — record every non-exact link
  with its grade. The counts below come from a snapshot taken four hours before
  the current HEAD, across a 209-file commit that moved markdown; treat them as
  the expected shape, not as ground truth, and reconcile any difference before
  drawing conclusions.
- [x] 3.2 State the criterion as a delta over that baseline so it survives future
  commits: every link that leaves the report must be one of the named five, no
  link may enter it that was not already non-exact, and the dangling count must
  strictly decrease.
- [x] 3.3 `kenn check links` returns exactly **one row** — the
  `[[feedback_no_version_bumps]]` wikilink in the archived
  `2026-05-26-store-schema-versioning/design.md`, which names a machine-local
  memory file and is genuinely broken.
- [x] 3.4 `indexers/frames.ts` from `indexers/kenn-dotnet/README.md` no longer
  appears in the report — it is graded `exact`.
- [x] 3.5 All five resolve to attachment nodes with canonical keys
  (`md:@attachment/{LICENSE-MIT,LICENSE-APACHE,docs,claude-plugins/kenn,crates/kenn-model}`).
  Correction to this task as written: each has **one** reference on this repo,
  not two — README links each licence once — so the two-spellings-collapse
  property is covered by unit test only. The reindex did surface a real defect:
  the first cut keyed `docs/` verbatim, minting `md:@attachment/docs/`. Fixed by
  routing both spellings through the join; the guard for it was initially
  vacuous because the test mock distinguished `docs` from `docs/` where the
  filesystem does not.
- [x] 3.6 Re-ran on `tmp/tsnest` (nestjs/nest): minted `md:@attachment/LICENSE`
  — the extensionless-licence class this change fixes — and left 14 rows that
  are same-file `#anchor` links (`[Question or Problem?](#question)` pointing at
  HTML anchors, not heading slugs). Those have an empty `RawLink::target`, which
  short-circuits in `resolve_link` before any code this change touches, so they
  are pre-existing anchor drift and out of scope.

## 4. Gates

- [x] 4.1 `cargo clippy --workspace --all-targets` — zero warnings.
- [x] 4.2 `just test`: 1420 passed across 60 suites. First run flaked on
  `kenn-store --test hybrid_search` (5 embedding failures, `left: 0` vectors) —
  pre-existing embedder contention under whole-workspace parallelism, not this
  change: the suite passes 6/6 in isolation and this diff touches no embedding
  code. Second full run clean.
- [x] 4.3 `just crap-ci` green; fix any new over-threshold function this change
  introduced by coverage or by splitting it, not by re-baselining.
- [x] 4.4 `cargo fmt --all` as the final step; commit unrelated formatting drift
  separately.

## 5. Code-review follow-ups

`/code-review` raised 14 findings against this change. Verified, then triaged.

- [x] 5.1 **Falsified gate claim.** Task 4.1's "clippy zero warnings" was stale:
  the gate ran *before* `cargo fmt --all`, and fmt re-wrapped a call that pushed
  `md_to_code_file_link_uses_links_to_file_edge` to 101 lines
  (`clippy::too_many_lines`). CLAUDE.md §7 puts fmt last; it does not say to
  re-run clippy after, and formatting can introduce warnings. Fixed by
  extracting `rust_file_batch`, and the gate order is now clippy → fmt → clippy.
- [x] 5.2 **Probe order was wrong** in `attachment_key`: root-relative was tried
  before the join, so `[the docs](docs)` in `crates/kenn-indexer/README.md`
  would bind to the repository-root `docs/`. Unlike `resolve_file_ref`, whose
  as-written probe hits a basename-filtered graph set, this one hits the whole
  filesystem where `docs`/`src`/`tests` exist at several depths. Joined now
  wins; guarded and mutation-verified (`left: Some("docs")`).
- [x] 5.3 **Windows escape.** `join_relative` splits on `/` only, so
  `..\..\secrets` survived as one opaque segment and `C:/x` would replace the
  base — both defeating the above-root guard once the path reached `Path::join`.
  Now rejected outright. This mattered only because this change is what first
  sends link targets to the filesystem.
- [x] 5.4 **Vacuous test.** `raw_link("[[gone]]")` is a target `extract_links`
  can never emit (it strips the brackets), so the assertion restated the
  `missing-file` case and could not go red for any wikilink change — §9 again,
  in my own test. Replaced with a real `RawLink { wikilink: true, .. }` covering
  both halves of the D4 wikilink rule.
- [x] 5.5 **Orphaned doc comment.** `FsPaths` was inserted under `mint_stub`'s
  doc comment, silently retitling it and leaving `mint_stub` undocumented.
- [x] 5.6 **Unnameable public signature.** `pub fn resolve_markdown_code` took
  `&dyn PathExists` from a `pub(crate) mod`. `relpath` is now `pub`.
- [x] 5.7 **Duplicated backing.** The change unified the trait but left
  `FsPaths` and `FsAssets` byte-identical in two modules. One `FsPaths` now
  lives beside the trait in `relpath.rs`.
- [x] 5.8 **Two overclaiming spec scenarios corrected.** (a) The delta asserted
  one markdown+HTML target collapses to a *single* node; the corpora key in
  their own namespaces (`md:@attachment/…` vs `html:…`), so it is one node per
  corpus. Unifying that means changing HTML's shipped stub ids — recorded as a
  separate change, not smuggled in. (b) "one href resolves its file target and
  its fragment identically" was untestable as written, because `fragment()`
  returns before file resolution; narrowed to the joining rule it actually
  pins.
- [x] 5.9 **`[t](LICENSE-MIT)` IS looked up as a code symbol** — the old
  scenario claimed otherwise. Under D4 that is correct (graph resolution
  precedes existence), so the spec now says so, plus a scenario for the
  collision case. See 5.10 for the part that is not yet settled.

### Settled

- [x] 5.10 **Bare-name shadowing fixed.** An inline `CommonMark` destination
  means a *path*, so a bare name the workspace holds now beats a same-named code
  symbol; a wikilink keeps symbol-first, which is its convention. Narrowed to
  `!is_code_path(target)` — a path-shaped target must still resolve to its
  indexed file node (the first cut broke exactly that). Guarding it took three
  attempts: unit guards on the helpers survived the mutation, and so did a
  store-backed test whose link was `../notes` — the slash routed it to the file
  branch, so the symbol lookup never ran. With a bare `notes` the mutation fails
  `left: 1, right: 0`.
- [x] 5.11 **md/html divergence closed.** `is_asset_ref` is deleted: existence
  decides on both sides, since `mint_asset` already dangles a target the
  workspace does not hold, so the gate only ever suppressed *existing* targets
  whose extension kenn indexes. Both corpora now kind a resolved target through
  one shared rule, `markdown::existing_target_kind` — `Document` for a `.md`/
  `.html` the config excluded, `Attachment` for binaries, extensionless files,
  and directories. An excluded `.md` is no longer recorded as a leaf.
- [x] 5.12 **Unverifiable anchors downgrade.** An attachment is not in the
  corpus, so its sections are unknown; `[notes](vendor/CHANGELOG.md#v1-0-0)` now
  grades `drifted`, mirroring `apply_anchor`'s "anchor present but unmatched →
  at least Drifted". Extracted as `attachment_grade` so it is directly
  testable.
- [x] 5.13 **Nits done.** The candidate probe is lazy and skips the second stat
  when both spellings are identical (a root-level linking file); the HTML stub
  builders share name derivation so `stub_kind`'s MIME guess is no longer run
  and discarded — which also fixed `attachment_symbol` passing a `pub_id` where
  a name was wanted.
- [x] 5.14 **Gate order corrected.** Re-ran clippy → fmt → **clippy** after all
  fixes: 0 issues, 1426 tests across 60 suites, CRAP passed, fmt clean,
  `check links` still 1 row with canonical keys.
