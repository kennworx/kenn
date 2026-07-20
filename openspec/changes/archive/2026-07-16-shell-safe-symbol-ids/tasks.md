## 1. Audit — ground the invariant across all six languages

- [x] 1.1 Indexed rust/ts/cs (this repo) + cloned fixtures swift-argument-parser,
      samber/lo, pallets/click. **Result table in `design.md`** (Constructor-kind
      open-question). Key findings: constructor complexity is **C#-only**; go + python
      are **already clean** (0 hostile); rust = backtick + `<>` + lifetimes; ts =
      backtick + `()`; swift = `()` (labels) + operators (`==`/`<`) and already appends
      a `#<hash>` overload disambiguator. Swift operator glyphs seen: `==`, `<` (more
      via `op_*` general rule).
- [x] 1.2 Safe alphabet + delimiter roles (+ `=` fn-return, `_word_` formers, `_x<NN>_`
      floor) recorded in `design.md` D1.
- [x] 1.3 Consumers inventoried: `markdown/walk.rs:279` (`ends_with`), `css/ingest.rs`
      (`rsplit(['.','/'])`), `html/links.rs` (`strip_prefix`) — all parse on safe
      separators. **`edge.rs:329` confirmed NOT a `pub_id` consumer** (reads
      `occ.symbol` via `splitn(5, ' ')`).

## 2. Shared escaper + per-language rendering

- [x] 2.1 Shared name/char **escaper** in kenn-model: `escape(&str) -> String` that
      maps `$`→`@` and floors every other shell-hostile byte to `_x<NN>_`, with **no
      structural swaps** (so it can never mis-represent a construct). Pure + unit
      tested. Each ingester calls it for leaf names / as the safety floor.
- [x] 2.2 Per-language render dispatch `render(Language, raw) -> String` (Rust side),
      each arm the language's own rules — correctness over uniformity:
      - rust: drop backticks, `<>`→`~`, drop lifetimes; go/python: identity (audit-clean).
      - ts: drop backticks, `()`→`+`.
      - csharp (interim string-level, Roslyn version is 3.1): `()`→`+`, `<>`→`~`,
        drop spaces, escape the rest.
      - swift: operator-glyph member names → `op_*` words (`<`→`op_lt`, `==`→`op_eq_eq`),
        `()`→argument labels. **Must not** treat swift `<` as a generic.
      - markdown: `escape` at its record constructors (done). css/html: `escape` too,
        but their `!unresolved` stub markers make it a test-updating change — deferred.
      Wire at the ingest seams (`stream.rs` has `self.language`; `walk.rs`/`naming.rs`
      have `language`; the md/css/html producers). Verify per-language: a swift `<`
      operator renders `op_lt`, NOT `~`.
- [x] 2.3 Conformance test (the invariant's teeth): reindex this repo (rust/ts/cs) and
      re-run the go/py/swift + markdown fixtures, scan every emitted `pub_id`, fail on
      any shell-hostile char. Verify: 0 hostile across all languages, and the swift
      operator spot-check passes. **Mutation-check (§9)**: reintroduce a raw backtick
      pass-through and confirm it fails.

## 3. Richer rendering (future) — still Rust-owned (D6)

Shell-safety is a Rust-ingestion concern; indexers emit real symbols and never learn
about shells (D6). So this phase is **not** "move rendering into the binaries" — that
was a misframe now corrected in D6. Rendering stays in `pubid::render`; the only thing
a sidecar may do is emit a *richer real symbol* so the Rust renderer has more to work
with. go/python need nothing.

- [ ] 3.1 *(future)* Prettier C# ids: `_word_` constructors (`_array_`/`_optional_`/
      `_tuple_`/…), `=` fn-return, external-leaf + leaf registry (D1/D3/D4). Done in
      `render_csharp` (Rust); where the current raw symbol can't disambiguate a
      construct (array vs tuple vs nullable), have **kenn-dotnet emit a richer real
      symbol** for `render_csharp` to interpret — kenn-dotnet still knows nothing about
      shells. Today's lossy floor is already safe + unique; this is readability only.
- [x] 3.2 *(dissolved)* No rendering "moves into a binary" — it was never meant to
      leave Rust (D6). Confirmed kenn-swift's existing `#<hash>` already covers
      type-only overloads (task 1.1); `render_swift` owns the shell-safe transform.

## 4. Confirm no consumer regresses

- [x] 4.1 Confirm consumer behavior is unchanged — NO code change to `edge.rs`.
      Verify: an edge-classification test over a callable + non-callable symbol
      matches pre-change results; the markdown suffix match (`markdown/walk.rs:279`)
      still resolves a link to a code symbol.

## 5. Migration + gates

- [x] 5.1 Confirm the `pub_id` move's blast radius. Verify: findings `.anchor.jsonl`
      records key on file paths (not ids) → anchors survive; the vector sidecar keys
      on a content fingerprint + short_id → vectors survive UNLESS `pub_id` is part of
      `embeddable_text` (check; if so bump `CODE_TEXT_RECIPE`). Record in `design.md`
      Migration.
- [x] 5.2 `kenn index --force` end-to-end on this repo; assert the atlas concept docs
      show safe, readable ids, and `kenn get <id>` resolves unquoted for a rust, a ts,
      and a csharp symbol.
- [x] 5.3 Gates: `cargo clippy --workspace --all-targets`, `just crap-ci`,
      `cargo fmt --all`; if `kenn-dotnet`/`kenn-ts`/`kenn-swift` touched, their
      format/test recipes (`dotnet format`, `bun test`, `just test-indexer-swift`,
      `just probe-smoke`).
