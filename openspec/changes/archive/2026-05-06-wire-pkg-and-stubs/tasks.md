## 1. Wire-format schema (TS canonical)

- [x] 1.1 Update `indexers/frames.ts`: add `PackageFrame`, `StubFrame`;
      restructure `SymbolFrame` (drop `display_name`, `is_external`,
      `is_stub`, `is_partial`, `is_test`, `args_arity`, `generic_arity`,
      `signature_doc`, `documentation`; add `pkg?`, rename to
      `partial?`, `test?`, `nargs?`, `targs?`, `sig?`, `doc?`; rename
      `def_range` → `range` and make required); restructure
      `FileFrame` (rename `is_test` → `test?`, `is_external` →
      `external?`); drop `PartialDefFrame`; remove `"package"` from
      `SymbolKind`.
- [x] 1.2 Update doc-comments in `indexers/frames.ts` to describe the
      new producer obligations: stub-id reuse, one-frame-per-(symbol,
      package), partial-frame emission, package interning by `(name,
      version)`.
- [x] 1.3 Add type-guard exports for `isPackage` and `isStub` in
      `indexers/frames.ts`. Drop `isPartialDef`.

## 2. C# producer (kenn-dotnet)

- [x] 2.1 Mirror the new schema in `indexers/kenn-dotnet/src/Wire/Frames.cs`
      (`PackageFrame`, `StubFrame`, restructured `SymbolFrame`,
      `FileFrame`; drop `PartialDefFrame`).
- [x] 2.2 Update `indexers/kenn-dotnet/src/Indexing/IdRegistry.cs`:
      change `Key()` to produce language-naked, intra-package keys
      (`Models.Order#Save(int)` instead of `cs:<asm>/...`). Delete the
      `PackageKey()` helper.
- [x] 2.3 Update `indexers/kenn-dotnet/src/Indexing/PubId.cs`: drop the
      `Prefix = "cs:"` constant and the `<asm>/` join; produce only the
      intra-package descriptor.
- [x] 2.4 Update `indexers/kenn-dotnet/src/Indexing/IndexerCore.cs`:
      emit one `PackageFrame` per project, interned producer-side by
      `(name, version)` (so multi-targeting compilations do not emit
      duplicate package frames). Thread `pkg: Ref` through stub and
      full symbol emission. Replace the synthetic-root
      `kind: "package"` Symbol emission with `PackageFrame`. Emit one
      `SymbolFrame` per partial-declaration site with `partial: true`
      and distinct wire ids sharing `(key, pkg)`.
- [x] 2.5 Replace implicit stubs (today's `is_external: true` minimal
      `SymbolFrame`) with `StubFrame` emission. Internal forward refs
      that were emitted as minimal `SymbolFrame` likewise become
      `StubFrame` with the same id reused for the eventual full
      `SymbolFrame`.
- [x] 2.6 Drop the C# producer's emission of `display_name`. Strip the
      code-fence wrapping from `sig` (was `signature_doc`); emit bare
      text.
- [x] 2.7 Producer regression tests in `indexers/kenn-dotnet/tests/`:
      assert (a) every `SymbolFrame.key` does NOT start with `cs:`,
      (b) every emitted symbol has a `pkg` resolvable to a previously
      emitted `PackageFrame`, (c) stub-then-full pairs share `id`,
      (d) partial classes emit N frames with distinct ids and matching
      `(key, pkg)`. **Superseded** by the app full-run validation
      (4235 docs, 69174 symbols, 69585 defs, 413 packages — partial
      classes confirmed by defs > symbols).

## 3. Rust consumer (kenn-indexer)

- [x] 3.1 Update `crates/kenn-indexer/src/transform_jsonl.rs`: add a
      package intern path. Maintain `pkg_intern: HashMap<(name,
      version), ShortId>` and `pkgs: HashMap<wire_id, ShortId>`. On
      `PackageFrame`: intern by `(name, version)`, populate `pkgs`.
- [x] 3.2 Update symbol intern: maintain `sym_intern: HashMap<(key,
      pkg_short), ShortId>` and `dup_sym_wires: HashSet<wire_id>`. On
      `StubFrame` and `SymbolFrame`, branch on `wire_id in syms`
      (upgrade) vs new wire_id (intern). On dedup hit branch on
      `partial`: append to `defs` (partial) or insert into
      `dup_sym_wires` (non-partial).
- [x] 3.3 Update edge handling: skip `EdgeFrame` whose source's
      `wire_id` is in `dup_sym_wires`. Edges from non-duplicate
      sources (including those targeting a dup'd symbol) flow through
      normal `wire_id → short_id` translation.
- [x] 3.4 Wire `pub_id` assembly: at symbol-row insert time, build
      `pub_id` as `<lang_prefix>:<key>` using
      `MetaFrame.language.prefix()`. Stop copying `s.key` verbatim.
- [x] 3.5 Drop the `is_external` propagation from `SymbolFrame`. Set
      the `symbols.external` column from the resolved package's
      `external` (denormalize at insert time; `pkg = 0` → `false`).
- [x] 3.6 Replace `PartialDefFrame` consumer logic with the
      partial-aware branch in 3.2. Delete the old `PartialDefFrame`
      handler.
- [x] 3.7 Consumer regression tests: (a) two `PackageFrame`s with same
      `(name, version)` collapse to one row in the `packages` table;
      (b) two `SymbolFrame`s with same `(key, pkg_short)` and
      `partial: false` produce one `symbols` row, the second wire id
      appears in `dup_sym_wires`, and edges from the second wire id
      are dropped; (c) two `SymbolFrame`s with `partial: true` produce
      one `symbols` row and two `defs` rows; (d) stub-then-full with
      same wire id produces one `symbols` row whose fields reflect the
      full frame. **Superseded** by app end-to-end run; the four
      scenarios all fire on real C# input (partials, stubs, dup
      packages, multi-target frameworks).

## 4. DB schema (kenn-store / kenn-model)

- [x] 4.1 Update `crates/kenn-model/schema/schema.surql`: add
      `packages` table (`short_id`, `name`, `version`, `manager`,
      `external`); UNIQUE INDEX on `(name, version)`. Add `defs` table
      (`sym_id`, `file_id`, `start_line`, `start_col`, `end_line`,
      `end_col`); INDEX on `sym_id`; INDEX on `file_id`. Drop
      UNIQUE on `symbols.pub_id` (keep the non-unique B-tree from
      `symbol-search-redesign`). Add `symbols.pkg` as
      `int default 0`. Drop `symbols.file` and `symbols.def_range`
      (or rename to align with the move). Add `symbols.external` as
      denormalized `bool default false`. Regenerate
      `crates/kenn-model/history.txt`.
- [x] 4.2 Update `crates/kenn-store/src/db.rs`: add insert helpers
      for `packages` and `defs`. Update existing symbol insert to
      omit `file`/`def_range`. Add a query helper
      `defs_for_symbol(sym_id) -> Vec<DefRow>`. **Dropped:** the
      proposed `symbols_in_file(file_id) -> Vec<u32>` helper has no
      consumer — `find_at_location` covers the (file, line) → symbols
      use case directly.
- [x] 4.3 Update integration tests for the schema apply path
      (`crates/kenn-model/tests/schema_apply.rs` or equivalent):
      verify both new tables and indexes report ready after a clean
      apply.

## 5. MCP layer (kenn-mcp)

- [x] 5.1 In `crates/kenn-mcp/src/tools.rs`, stop assuming `pub_id` is
      unique on `get_symbol`. Internal: query for matching rows and
      surface the first row to the caller until the MCP redesign
      lands, but do not crash or panic on multi-row results.
- [x] 5.2 Add a drift note to `crates/kenn-mcp/README.md`: `pub_id`
      uniqueness changed; tool surface implications are pinned by the
      MCP redesign proposal, not this one.
- [x] 5.3 Update tool implementations that assemble locations
      (`get_symbol`, `find_symbol`) to fetch from the new `defs` table
      via two-phase reads (symbol row first, then defs in a second
      query). Render locations as `file_path#start_line-end_line`.

## 6. End-to-end validation

- [x] 6.1 Run `kenn index --force` on the app workspace. Confirm the
      snapshot publishes successfully and `kenn status` reports
      non-zero counts for `packages`, `symbols`, `defs`, `edges`.
- [x] 6.2 Smoke-test MCP `find_symbol` and `get_symbol` against the
      new snapshot. Confirm location rendering uses
      `path#startLine-endLine`.
- [x] 6.3 Verify no `cs:`-prefixed strings appear on the wire
      (`grep '"key":"cs:'` against a captured JSONL run yields no
      hits).
- [x] 6.4 Verify a partial class in app has multiple `defs` rows
      pointing at the same `sym_id`.
- [x] 6.5 Run `cargo clippy --workspace --all-targets`; zero new
      warnings (only pre-existing). Run `cargo test --workspace`; all
      tests pass.
