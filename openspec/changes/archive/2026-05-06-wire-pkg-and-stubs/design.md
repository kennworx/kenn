## Context

Today's wire format conflates two distinct identity layers. The producer
emits a `SymbolFrame.key` like `cs:App.Trading.Risk/Models.Order#Save(int)`,
and the consumer copies that string verbatim into the DB's `pub_id`
column (`crates/kenn-indexer/src/transform_jsonl.rs:198–203`). This means
the language prefix and the assembly are decided by the producer even
though they are concerns of layers above:

- The language is already declared once in `MetaFrame.language` — every
  symbol in the stream is in that language. Tagging each symbol with
  `cs:` is redundant on the wire.
- The assembly name is repeated in every symbol key. For a workspace of
  ~50k symbols with average assembly name ~20 bytes, that is ~1 MB of
  redundant string content per snapshot.
- The `pub_id` shape baked into the producer's keys forces the consumer
  to use that shape. If the consumer wants a different identity layout
  for any reason (e.g., a separate column for the assembly to enable
  `pkg`-filtered queries), it cannot get there without re-parsing the
  string.

Beyond the prefix issue, "package" is implicit on the wire: each C#
assembly gets a synthetic `kind: "package"` Symbol with key
`pkg:<asm>`, but no other symbol references it (no `parent` edge, no
`pkg: Ref`), so it is dead weight that exists only to satisfy a
SymbolKind variant and a pseudo-prefix convention.

Stub-vs-full frames are also implicit. The current spec describes a
"stub-then-upgrade" rule keyed on wire id (`SymbolFrame` UPSERT-by-id),
where minimum-info frames are inferred from field presence. There is no
explicit marker, so a consumer cannot validate "did the producer mean
this as a stub or a full record" — it must guess.

`PartialDefFrame` exists today to record additional declaration sites
of a partial class. It is a third frame type that overlaps in role with
the consumer-side dedup we now want for cross-wire-id collapse: both
need "this symbol I already saw, here is more about it." With the new
dedup pipeline, partials become a special case in the same logic.

## Goals / Non-Goals

**Goals.**

- Strip presentation concerns (language prefix, assembly path, code
  fences) from the wire. The wire carries structure; the consumer
  renders identity.
- Make "package" a first-class wire entity, normalized like files.
- Make stub-vs-full explicit on the wire so the consumer can validate.
- Reduce per-symbol byte cost by (a) replacing assembly substrings with
  `Ref` indirection and (b) making boolean flags optional with default
  false.
- Unify cross-wire-id dedup (multi-target collapse) and partial-class
  modeling under one consumer logic path keyed on `partial: true`.
- Keep the schema flexible for genuinely-different versions of the same
  named package (`Newtonsoft.Json` v12 and v13) — they are separate
  rows distinguished by `pkg`, not collapsed.

**Non-Goals.**

- Versioning the wire format. The project is prototyping; old snapshots
  are not preserved across the change. `WIRE_VERSION` stays at 1; we do
  not introduce a v2 path.
- Stabilizing the MCP surface. `get_symbol` and `SymbolRef` shapes
  shift as a side effect; the in-flight `mcp-server` redesign owns
  reconciling them. This proposal carries a drift note only.
- Source-vs-runtime variant tracking. Same library compiled to multiple
  TFMs is treated as one source; producer collapses TFMs at the
  package layer (interned by `(name, version)`).

## Decisions

### Decision 1: Replace `cs:<asm>/<path>` keys with structured fields

**What.** `SymbolFrame.key` becomes a language-naked, intra-package
path. The package is referenced via `SymbolFrame.pkg?: Ref`, pointing
at a `PackageFrame` previously emitted in the stream. The language
prefix is added consumer-side from `MetaFrame.language` when assembling
`pub_id`.

**Why this over alternatives.**

- *Keeping `cs:` on the wire* — would preserve the existing
  identity-mapped consumer behavior but pays the redundancy cost
  per-symbol forever. A single-language stream does not need
  per-symbol language tagging.
- *Embedding the assembly as a string but stripping `cs:`* — half-measure;
  still pays ~1 MB redundancy on app; still bakes presentation into
  the producer.
- *Using `<lang>:<asm>:<path>` with `Ref` only for the language* — adds
  string complexity for no win; we already have `Ref`s for files.

**Tradeoff.** Producer becomes slightly more complex (must emit
`PackageFrame` and resolve `pkg: Ref` at symbol emission time).
Consumer becomes slightly more complex (must intern packages and
assemble `pub_id`). The complexity is bounded and reusable across
languages; the byte and conceptual savings compound per symbol.

### Decision 2: One `SymbolFrame` per `(symbol, package)` instance

The wire emits one `SymbolFrame` per `(symbol, package)` combination,
with single `pkg: Ref`. Cross-package symbols in the same logical
sense (e.g., `System.Collections` namespace contributed to by N
assemblies) are emitted as N frames sharing `key` and `pkg.name`-but-
not-version, distinguished only by `pkg: Ref`.

**Alternatives considered.**

- *Array `pkg: Ref[]`* — was the initial design candidate. Rejected
  because it pushes producer-side aggregation logic across compilations
  ("walk all assemblies, then emit one frame per logical symbol with
  the union of packages") which conflicts with the streaming,
  single-pass design — the producer would need to buffer every namespace
  symbol until it has seen every assembly. Single `pkg` keeps the
  producer's model simple: one walk per compilation, one frame per
  symbol-in-this-compilation.
- *Unify across packages at the consumer (collapse `Newtonsoft.Json` v12
  and v13 into one logical row)* — rejected because edges from one
  consumer's call site bind to a specific version; collapsing loses
  per-caller version binding. With separate rows the call graph stays
  faithful: an edge target points at the row in the version actually
  resolved by that caller's compilation.

### Decision 3: Explicit `StubFrame` instead of an `is_stub` flag

A new top-level frame type `StubFrame` carries the minimum a consumer
needs to allocate a `short_id` and intern by `(key, pkg)`:

```
{ type: "stub", id, kind, name, key, pkg? }
```

`SymbolFrame` always means full record. The producer obligation is
explicit: when both forms are emitted for one symbol, the same `id`
is used; the consumer keys upgrade-vs-dedup off wire-id collision.

**Alternatives considered.**

- *Boolean `stub?: bool` on `SymbolFrame`* — works but leaves a fat
  frame carrying mostly-omitted fields when used for stubs. A
  dedicated frame is thinner and self-documents intent.
- *Implicit (current spec)* — relies on consumers parsing field
  presence. Brittle and ambiguous when a producer happens to omit a
  field for a non-stub reason.

### Decision 4: Partials as dedup-with-append, no separate frame

`PartialDefFrame` is dropped. Partial classes/methods are emitted as N
`SymbolFrame`s with `partial: true`, distinct wire ids, identical
`(key, pkg)`. The consumer's dedup logic branches on `partial`:

```
on dedup-hit (sym_intern (key, pkg_short) Occupied):
  if partial == true:
    insert defs row (sym_id=existing, file, start_line, ...)
    do NOT add to dup_sym_wires        // edges from this declaration
                                       // are legitimate (members called
                                       // from this specific file)
  else:
    dup_sym_wires.insert(wire_id)      // multi-target collapse;
                                       // edges are duplicates of the
                                       // first sighting's edges
```

The `partial` flag is the discriminator between two consumer outcomes
("append additional def" vs "skip duplicate edges") that both arise
from the same dedup-key collision.

**Alternatives considered.**

- *Keep `PartialDefFrame`* — preserves a structural distinction the
  spec already had, but means partials and multi-target dedup use
  different mechanisms even though both express "same canonical entity,
  more info." Unifying simplifies the consumer.
- *One `SymbolFrame` with array of declaration sites* — rejected for
  the same reason as Decision 2's array-pkg alternative: requires
  producer-side aggregation.

### Decision 5: `defs` as a separate table, not array column

Declaration locations move from `symbols.{file, def_range}` into a
separate `defs` table `(sym_id, file_id, start_line, start_col,
end_line, end_col)`.

**Why a side table over an array column.**

- Lines and columns are separate columns; the common
  `path#start_line-end_line` rendering projects three columns
  (`file_id`, `start_line`, `end_line`) and never touches column data.
- Append-on-partial-dedup is a single `INSERT INTO defs ...`. Array
  columns require atomic `array += $value` updates and lose the
  natural scoping of "which declaration site arrived first."
- The "list symbols in file F" query is a simple `SELECT DISTINCT
  sym_id FROM defs WHERE file_id = $f`, indexed cleanly. Array-element
  indexes work but read less naturally.
- Two-phase fetch is the consumer's standard pattern: find symbols
  (small set), then fetch ranges only when needed. Joins are not
  required and the design assumes none.

### Decision 6: Drop `pub_id` UNIQUE constraint

Different versions of the same logical package can declare symbols
sharing the same `pub_id` (e.g., `Newtonsoft.Json` v12 and v13 both
have `cs:Newtonsoft.Json.JsonConvert`). They are separate rows
disambiguated by `pkg`. There is no DB-enforced uniqueness; the
producer is expected not to emit duplicate `(pub_id, pkg)` pairs, and
the consumer's intern logic enforces it at ingest by collapsing
duplicates onto the first sighting (with the partial-aware branch).

`get_symbol(pub_id)` therefore becomes a query that may return multiple
rows. The MCP surface decision (return all and let the agent pick vs
require `pkg` to disambiguate) is deferred to the MCP redesign — at
the data-model layer we record only that uniqueness is `(pub_id, pkg)`,
not `pub_id` alone.

### Decision 7: Field renames and code-fence stripping

- `is_*` boolean flags become `*?` (optional, default false). Saves
  bytes and matches the more general "structured nulls" convention.
- `def_range` becomes `range` — there is no longer ambiguity between
  declaration range and edge range, since `EdgeFrame` already used
  `range`.
- `signature_doc` becomes `sig`, emitted as bare text (no code fence).
  Fencing is a presentation choice; if the MCP surface wants to render
  a fenced block it adds the fence based on `MetaFrame.language`.
- `display_name` is dropped. Consumers render from `name` (short) and
  `sig` (declaration line). The previous `display_name` mostly
  duplicated the first line of `sig`.

## Risks / Trade-offs

- **Multi-package `pub_id` collisions in practice.** Most workspaces
  rarely import multiple versions of the same package, but transitive
  NuGet deps can cause it. The consumer must handle "two rows with
  same `pub_id`" gracefully; debug tooling that assumed uniqueness will
  break. Mitigation: surface `pkg` in any debug print of a symbol.
- **MCP drift.** `get_symbol` returning multiple results changes the
  agent-facing tool contract. Mitigation: defer the surface change to
  the in-flight MCP redesign. Until then, MCP can return the first
  match; the data model supports the multi-match case when the surface
  catches up.
- **Producer must intern packages by `(name, version)`.** If the
  producer is naive across compilations (multi-targeting), the
  consumer's intern catches it but at the cost of redundant on-wire
  PackageFrames. Acceptable but inefficient at scale; the producer
  should intern producer-side too.
- **`partial: true` carries extra semantics.** It is now both "this is
  a partial declaration" and "consumer should append on dedup hit
  rather than skip edges." The two meanings happen to align in
  practice (partial classes do receive multiple legitimate emissions),
  but a producer that erroneously sets `partial: true` on a
  non-partial multi-target dedup case would cause edge duplication.
  Mitigation: spec language is explicit; producer test asserts the
  flag is set only for `IsPartial` symbols at the source level.
- **Reindex required.** All snapshots become unreadable. Acceptable
  given the prototype stage and absence of real users.

## Migration Plan

No data migration. Reindex via `kenn index --force` produces snapshots
under the new schema. The CLI surfaces no special command — the change
is detected from the schema version embedded in the snapshot. Old
snapshots are deleted by GC on the second post-change reindex.
