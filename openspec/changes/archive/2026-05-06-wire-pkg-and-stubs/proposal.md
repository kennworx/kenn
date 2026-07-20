## Why

The JSONL wire format embeds presentation concerns into producer-emitted
strings: every `SymbolFrame.key` carries a `cs:<asm>/` prefix, which (a)
duplicates the language already declared in `MetaFrame`, (b) repeats the
assembly name in every symbol (~1 MB of redundant bytes for a workspace
the size of app), and (c) hard-codes a `pub_id` shape that should be
the consumer's choice, not the producer's. The "package" concept is also
missing as a first-class wire entity — assemblies live as embedded
substrings in symbol keys plus a synthetic `kind: "package"` Symbol-root
hack per assembly. Stub-vs-full frames are implicit (the consumer
infers from which fields are populated). Partial-class declarations
require a separate `PartialDefFrame` whose role overlaps with the
dedup behavior we now want from the consumer ingest pipeline.

## What Changes

- **BREAKING (wire):** add `PackageFrame` as a top-level frame. Symbols
  reference their owning package via `SymbolFrame.pkg?: Ref` instead of
  embedding the assembly name in `key`.
- **BREAKING (wire):** strip the `<lang>:` prefix from `SymbolFrame.key`.
  Keys are language-naked, intra-package paths
  (`Models.Order#Save(int)`). The consumer assembles `pub_id` as
  `<lang_prefix>:<key>` using the language declared in `MetaFrame`.
- **BREAKING (wire):** introduce explicit `StubFrame` for forward refs
  and external symbols. Drop `SymbolFrame.is_stub` (would-be) and the
  implicit "stub-vs-full from field presence" inference. Producers MUST
  use the same `id` for a `StubFrame` and any subsequent `SymbolFrame`
  that completes it.
- **BREAKING (wire):** drop `PartialDefFrame`. Partial declarations are
  modeled as multiple `SymbolFrame`s with `partial: true`, distinct wire
  ids, and the same `(key, pkg)`. The consumer's dedup branch on
  `partial: true` appends additional declaration sites; on `partial:
  false` (multi-target dedup) it skips edges from the duplicate.
- **BREAKING (wire):** drop `SymbolFrame.is_external` (consumer derives
  it from `pkg.external`); drop `SymbolFrame.display_name` (consumers
  render from `name`/`sig`). Drop `kind: "package"` from `SymbolKind`.
- **BREAKING (wire):** rename booleans to drop the `is_` prefix and make
  them optional with default false: `is_partial → partial?`, `is_test →
  test?`, `is_external → external?`. Rename `signature_doc → sig`,
  `documentation → doc`, `def_range → range` (now required on
  `SymbolFrame`), `args_arity → nargs?`, `generic_arity → targs?`. Drop
  the code-fence wrapping convention on `sig` — emit bare text;
  presentation belongs to the consumer.
- **BREAKING (DB):** add a `packages` table interned by `(name,
  version)`. `symbols.pkg` becomes a non-nullable u32 column with `0`
  as the "no package" sentinel. `symbols.pub_id` loses its UNIQUE
  constraint — duplicates across packages (same name, different package
  version) are stored as separate rows and disambiguated by `pkg`.
- **BREAKING (DB):** move declaration locations off the symbol row into
  a separate `defs` table `(sym_id, file_id, start_line, start_col,
  end_line, end_col)`. One row per declaration site (1 in the common
  case, N for partial classes). Lines and cols are separate columns so
  callers can project the line subset for `path#L-L` rendering without
  fetching column data.
- **Consumer:** ingest dedups packages by `(name, version)` and symbols
  by `(key, pkg_short)` via the existing `wire_id → short_id`
  translation map plus a small `dup_sym_wires: HashSet<wire_id>` that
  drives edge-skip decisions. Partial-aware branch on dedup hit appends
  to `defs` instead of skipping edges.

## Capabilities

### Modified Capabilities

- `dotnet-stream-indexer` (in-flight, owned by this proposal): the
  wire-format spec gains `PackageFrame` and `StubFrame`, drops
  `PartialDefFrame`, restructures `SymbolFrame`. The producer
  obligations around stub-id reuse and partial emission are added.
- `index-store-db` (in-flight, owned by `indexed-store-and-lifecycle`):
  the schema gains a `packages` table and a `defs` table, drops the
  `symbols.pub_id` UNIQUE constraint, denormalizes
  `symbols.is_external` from `packages[pkg].external`, and removes the
  inline `file`/`def_range` columns from `symbols`.
- `mcp-server` (in-flight, owned by `mcp-server` proposal): drift note —
  `get_symbol(pub_id)` may return multiple rows when packages differ;
  `SymbolRef` envelopes carry the resolving package so the agent can
  disambiguate. Concrete API shape is deferred to the in-flight MCP
  redesign and is not pinned here.

## Impact

- **Code:**
  - `indexers/frames.ts` — schema rewrite (add `PackageFrame`,
    `StubFrame`; restructure `SymbolFrame`; drop `PartialDefFrame`;
    field renames).
  - `indexers/kenn-dotnet/src/Wire/Frames.cs` — mirror schema in C#.
  - `indexers/kenn-dotnet/src/Indexing/IndexerCore.cs` — emit
    `PackageFrame` per project (interned by name+version producer-side
    so multi-target compilations do not duplicate); thread `pkg`
    through symbol emission; emit one `SymbolFrame` per partial
    declaration site.
  - `indexers/kenn-dotnet/src/Indexing/IdRegistry.cs`,
    `indexers/kenn-dotnet/src/Indexing/PubId.cs` — rewrite key
    generation to produce language-naked, intra-package keys; delete
    the `pkg:<asm>` synthetic-root helper.
  - `crates/kenn-indexer/src/transform_jsonl.rs` — package interning
    by `(name, version)`, symbol dedup by `(key, pkg_short)`,
    partial-aware append branch, edge-skip via `dup_sym_wires`.
  - `crates/kenn-store/src/db.rs` and
    `crates/kenn-model/schema/schema.surql` — `packages` table,
    `defs` table, schema changes on `symbols`.
  - `crates/kenn-mcp/src/tools.rs` — internal: stop assuming
    `pub_id` uniqueness on `get_symbol`; pass through `pkg` data on
    `SymbolRef`. External MCP-surface change deferred to MCP redesign.
- **APIs:** wire-format breaking. C# producer, Rust consumer, and any
  in-tree TS schema consumers must update in lockstep.
- **Schema:** breaking. No data migration — reindex with `kenn index
  --force` produces new snapshots; old snapshots are no longer readable
  by the new consumer.
