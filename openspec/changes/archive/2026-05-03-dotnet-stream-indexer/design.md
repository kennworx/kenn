## Context

Today's C# index path: `code-intel-indexer::CSharpScipDotnet` runs the
upstream `scip-dotnet` binary, which uses Roslyn + MSBuildWorkspace to walk
every project and writes a single SCIP protobuf file (~hundreds of MB on
app). The Rust pipeline then opens that file, runs a streaming parser,
applies a per-language transform (`crates/code-intel-indexer/src/transform.rs`)
to produce `FileRecord` / `SymbolRecord` / `SymbolDocsRecord` / `EdgeRecord`,
and pushes them through `BatchingSink<SurrealdbSink>` into embedded SurrealDB.

This works but loses information at the SCIP layer. `SymbolRecord.def_range`
is always `[0,0,0,0]` (B2 in `scratch/next-task-notes.md`) because
`transform_document` never reads `Definition`-role occurrences back into
the symbol row. `packages: []` for all C# workspaces (B3) because
`scip-dotnet` emits `Kind::Namespace` and our `distinct_packages` query
filters on `Kind::Package`. Both are artifacts of going through SCIP's
generic occurrence model when Roslyn already has the answer.

The user has scoped a from-scratch prototype that bypasses SCIP entirely
for C#: a new .NET 10 console app emits our own JSONL wire format on
stdout, and a new ingest mode in `scratch/surreal-spike` consumes it.
Production code under `crates/` is untouched. Promotion is a follow-up.

## Goals / Non-Goals

**Goals:**

- Replace SCIP protobuf as the C#-side serialization with a JSONL wire
  format consumed straight from a pipe. No `.scip` file on disk.
- Fix B2 by emitting `def_range` directly from
  `ISymbol.Locations[0].GetMappedLineSpan()`.
- Fix B3 by synthesizing one root package symbol per assembly with
  `kind: package`.
- Cover the v1 narrow edge set first (`defined_in`, `contains`,
  `implements`, `overrides`, `calls`), then expand to app parity within
  this same change.
- Walk into method/lambda bodies for call/reference edges with the
  enclosing fn/method/class as the source.
- Land a tokenizer-probe findings file that informs a separate B1 fix
  proposal.
- Prove the prototype on the app workspace end-to-end (index → SurrealDB
  → simple queries returning expected results).

**Non-Goals:**

- Wiring this prototype into `code-intel-indexer::CSharpScipDotnet`. That
  is a follow-up after the prototype proves out.
- Any change to production crates (`crates/code-intel-*`,
  `crates/source-model`).
- Other languages (TypeScript, Rust, Go, Python). They keep using
  scip-* drivers and the existing transform.
- The actual B1 (search analyzer) fix. That's its own proposal; this
  change only produces the probe findings.
- Cross-snapshot stable IDs, file watchers, CLI lifecycle (lock,
  publish, GC). Out of scope for the prototype.
- Incremental indexing. Single-pass walk per run.

## Decisions

### D1 — JSONL envelope with `type` discriminator

Each line is `{"type": "<kind>", ...payload}`. Six kinds: `meta`, `file`,
`symbol`, `partial_def`, `edge`, `end`.

Alternatives considered:
- **Length-prefixed binary protobuf** — fastest, smallest, lossless, and
  reuses prost types in Rust. Rejected: user explicitly wants no protobuf
  in the dotnet indexer, and JSONL is debuggable by piping to `head`/`jq`.
- **Canonical proto-JSON of `Scip.Document`** (via
  `Google.Protobuf.JsonFormatter`) — cheapest C# emit, but ties us back to
  SCIP's generic shape and requires a serde-mirror struct in Rust.
  Rejected: defeats the goal of a minimal schema we control.
- **Tag in payload (no discriminator)** — relying on field shape to tell
  frame types apart. Rejected: brittle, harder to extend.

Why `type`, not `kind`: `kind` collides with the `Kind` enum on symbol
frames. `type` is unambiguous and matches `serde(tag = "type")` defaults.

### D2 — `pub_id` is the only cross-reference identifier on the wire

The producer never sees Rust's `short_id` (assigned by the consumer's
`IdRegistry`). All edges and parent links use the `cs:`-prefixed
`pub_id` string.

Format follows scip-dotnet's existing scheme but shorn of the SCIP
descriptor syntax:
- Package: `cs:pkg/<AssemblyName>`
- Namespace: `cs:<DottedName>` (e.g., `cs:MyApp.Services`)
- Type: `cs:<Namespace>.<TypeName>` (e.g., `cs:MyApp.Foo`)
- Method: `cs:<Namespace>.<TypeName>#<MethodName>(<sig>)` with overload
  disambiguator built into `<sig>` so two overloads of `Bar` get distinct
  ids (`Bar()` vs `Bar(int)`).
- Field/Property: `cs:<Namespace>.<TypeName>#<MemberName>`

Alternatives:
- **Reuse SCIP descriptor syntax verbatim** — would let us share the
  existing csharp transformer's parser. Rejected: that parser is the SCIP
  bridge we're trying to remove. The new format can be parsed with
  trivial split-on-`#` logic.
- **Hashes/UUIDs** — opaque, not human-debuggable. Rejected.

### D3 — Forward references are allowed; consumer stubs on first sight

The producer streams in document order, which means an edge in document A
can reference a symbol declared in document B (read later). The consumer's
`IdRegistry` registers any unknown `pub_id` from an edge as a stub
`SymbolRecord` with `is_external: true` and a placeholder `def_range`,
assigns a `short_id`, and patches the row when the real `symbol` frame
arrives.

This matches how the existing Rust pipeline already handles SCIP
occurrences. Single-pass streaming preserved.

True externals (NuGet types referenced but not in source) keep
`is_external: true` permanently.

### D4 — Edges carry their own range; no separate occurrence table

Every reference (call, field access, type use) emits one `edge` frame
with a `range` field. There is no separate "occurrences" record class.
This matches the `EdgeRecord` shape already in `crates/source-model`
(which carries the range in its payload). Our `transform_document`
already does this collapse for SCIP; we're just doing it directly in C#.

### D5 — Walk method/lambda bodies, but locals are not symbols

Roslyn semantic models give us call sites for free if we walk
`SyntaxNode`s and ask `SemanticModel.GetSymbolInfo(node).Symbol` per
identifier. The walker emits an `edge` for each invocation /
member-access / object-creation / type-reference whose source is the
nearest enclosing fn/method/class.

Locals (`SymbolKind.Local`, `Parameter` of lambdas, `RangeVariable`,
anonymous types, lambda methods themselves) are filtered out at emission
time. They never become `symbol` frames, never become edge sources or
targets. Roslyn's `IsLocalSymbol` predicate (lifted from
scip-dotnet/ScipDocumentIndexer.cs) is the gate.

### D6 — Synthetic root package, plus `defined_in` for top-level namespaces

Per assembly built from source:
1. Emit one `symbol { type:"symbol", kind:"package",
   pub_id:"cs:pkg/<AssemblyName>", enclosing_symbol omitted }`.
2. For each top-level namespace declared in that assembly, emit one
   `edge { edge_kind:"defined_in", source_pub_id:"cs:<NS>",
   target_pub_id:"cs:pkg/<AssemblyName>" }`.

`distinct_packages` (currently `WHERE kind = 'package' AND
enclosing_symbol = 0`) then returns one row per assembly. No SurrealDB
query change needed.

We deliberately do NOT switch the producer to "namespaces are packages":
that would conflate two real concepts and confuse multi-language
queries.

### D7 — Hex-string `content_hash`, never u64

JSON integers are not safe past 2^53. Producer computes xxh64 of the
file's UTF-8 bytes, formats as 16 lowercase hex chars, emits as a
string. Consumer parses with `u64::from_str_radix(s, 16)`.

### D8 — Documentation inline on the symbol frame; consumer splits

`signature_doc` and `documentation` are optional fields on the `symbol`
frame. Producer omits them when both are empty. Consumer emits a
`SymbolDocsRecord` only when at least one is non-empty.

This shaves one frame per symbol-without-docs (the common case) versus a
separate `symbol_docs` frame per symbol.

### D9 — .NET 10 SDK pin, MSBuildLocator at runtime

`scratch/dotnet-stream/global.json` pins SDK to `10.0.0` (matches
`scratch/scip-dotnet/global.json`). `Program.Main` calls
`MSBuildLocator.RegisterDefaults()` before any MSBuild assembly loads,
exactly as scip-dotnet does. Required so the Roslyn / MSBuild assemblies
resolve against the on-machine SDK rather than the app's own `bin/`
copies.

### D10 — `dotnet restore` and target-framework dedupe inherited from scip-dotnet

We re-implement (don't import) the small bits of `ScipProjectIndexer`
that handle:
- Running `dotnet restore` per project (with `--skip-dotnet-restore`
  flag to disable).
- Skipping projects whose `Language` is not `"C#"` (we don't ship VB
  support in v1).
- The TFM dedupe: if MSBuild returns one `Project` per target
  framework, prefer the `(net10.0)` one and skip the rest.

This is straight-up Roslyn API usage; no novel design.

### D11 — Consumer ingest mode in surreal-spike

Add subcommand `spike ingest-jsonl` that reads stdin (or a file via
`--input`), parses lines as `Frame`, runs the resolve+batch loop, and
writes to embedded SurrealDB via the existing `bulk_insert_*` helpers
(or new ones; whichever is cleaner).

CLI flags (per spec): `--db <path>` (required), `--batch-size <int>`,
`--reset-db`, `--input <path>`, `--quiet`/`--progress`. Implemented with
plain manual arg parsing (matching surreal-spike's existing style;
already does this in `print_help` / `parse_iter`).

Schema setup uses `schema_phase1` / `schema_phase2_fts` already in
surreal-spike. Reuse, don't rewrite.

### D14 — Producer CLI argument surface

Argument parsing uses `System.CommandLine` (same as scip-dotnet) so we
get `--help`, validation, and required/optional handling for free.
Required flags per spec:

- `--workspace <dir>` (default cwd) — workspace root.
- `--projects <path>...` — explicit `.sln`/`.csproj` list (zero-or-more).
- `--include <glob>...`, `--exclude <glob>...` — file path filters
  (Microsoft.Extensions.FileSystemGlobbing, same as scip-dotnet).
- `--skip-restore` (bool) and `--restore-timeout-ms <int>` (default
  300000) — `dotnet restore` controls.
- `--flush-bytes <int>` (default 1048576) and `--flush-frames <int>`
  (default 4096) — output batching thresholds.
- `--edge-kinds <list>` — comma-separated edge-kind allowlist (default:
  all supported).
- `--output <path>` — optional tee target (for debugging; stdout is
  always emitted).
- `--log-level <enum>` — log level for stderr-routed logger.

Stderr is the only logging channel; stdout is reserved for JSONL.

### D15 — No `--include-locals` flag

We deliberately do not expose locals-as-symbols even behind a flag.
"Semantic navigation, not source-code details" is a hard product
boundary, and adding the toggle invites cross-cutting bugs in the
consumer (records that exist in some runs but not others). If we ever
want this for debugging, it can be reconsidered then.

### D12 — End-to-end demo command shape

```sh
cd /path/to/workspace
dotnet run --project /…/scratch/dotnet-stream -- index . \
  | cargo run --manifest-path /…/scratch/surreal-spike/Cargo.toml \
      --release -- ingest-jsonl --db /…/scratch/surreal-spike/db-jsonl
```

Wall-clock and DB size are reported in the consumer at `end` frame.

### D13 — B1 tokenizer probe lives next to its findings

The probe is a small shell+SurrealQL script (or a one-off `surreal sql`
session) run against an existing `.code-intel/snapshots/<latest>` from a
prior app index, OR against the prototype's own
`scratch/surreal-spike/db-jsonl` once it has data. Output committed at
`scratch/b1-tokenizer-findings.md`. Findings inform the separate B1 fix
proposal.

We do not block the prototype on having a fresh snapshot — any existing
indexed C# snapshot in `.code-intel/` works.

## Risks / Trade-offs

[**Roslyn walk re-derives what scip-dotnet already does**] → Mitigation:
copy the small useful bits (`IsLocalSymbol`, TFM dedupe, restore loop)
verbatim from scip-dotnet sources. Don't re-research these. Reference
files are already in `scratch/scip-dotnet/`.

[**JSONL is 3–5× larger than binary protobuf**] → Mitigation: pipe
throughput is irrelevant compared to Roslyn analysis time. Memory footprint
is per-line, not per-index, so it stays bounded regardless of size.

[**Forward references mean stubs may persist if a real symbol frame
never arrives**] → Mitigation: that's actually the correct semantics —
those *are* externals (NuGet, framework). The consumer marks them
`is_external: true` and that flag is what `is_external` is for.

[**Stdout buffering on .NET swallows the streaming benefit**] →
Mitigation: producer-side batching with explicit thresholds (default
1 MiB or 4096 frames, whichever first; configurable via
`--flush-bytes` / `--flush-frames`). The producer flushes when a
threshold trips, on `end`, and on process exit. Per-line flushing was
considered and rejected as wasteful — pipe/syscall overhead per frame
adds up across millions of frames, and the consumer can't keep up with
single-frame round-trips anyway. Batched flushes deliver bytes in
useful chunks while keeping memory footprint bounded.

[**Two parallel Roslyn-using binaries (scip-dotnet and dotnet-stream)
on PATH may confuse the existing CSharpScipDotnet driver**] →
Mitigation: this prototype is invoked manually for now; the existing
driver continues to look for `scip-dotnet` and is untouched.

[**Edge coverage v1 ships before parity expansion lands**] → Accepted.
`tasks.md` sequences narrow → expand → probe so a halfway-done state is
still useful for B2/B3 demonstration. Each expansion edge-kind is its
own task.

[**Pub_id format choice is permanent**] → Mitigation: explicit spec
requirement (`Stable pub_id Scheme for C#`) with overload-disambiguation
scenario. If we change the scheme later, every snapshot reindexes —
which is fine because the prototype DB is throwaway, but worth flagging
as a semver-style commitment when we promote.

[**Same-process RocksDB lock leakage** documented in next-task-notes.md
item K] → Mitigation: prototype consumer never reads from the same DB
it writes to within one process; the writer process exits before any
read-side spike runs.

## Migration Plan

This is a prototype under `scratch/`. Nothing migrates *into* this
change. After it lands, a follow-up change will be proposed to wire the
new producer into `code-intel-indexer::CSharpScipDotnet` (replacing the
file-write/file-parse with a piped subprocess), at which point a
migration plan for production becomes meaningful.

Rollback for this change: delete `scratch/dotnet-stream/` and the new
`ingest-jsonl` subcommand from surreal-spike. Production crates are
untouched, so production has nothing to roll back.

## Open Questions

- **Q-OPEN-1**: Should the producer emit `enclosing_range` (broader span
  containing the whole symbol body) in addition to `def_range` (just the
  identifier)? `SymbolRecord` doesn't have such a field today. Defer
  unless a consumer query needs it.
- **Q-OPEN-2**: For partial classes, is one `symbol` frame + N
  `partial_def` frames the right model, or one frame per declaration
  site? Going with the former; revisit if it complicates DB queries.
- **Q-OPEN-3**: How do we attribute calls inside auto-property
  initializers and primary-constructor bodies (C# 12+)? Roslyn surfaces
  these as their own `IMethodSymbol`s; we'll attribute to the synthetic
  method symbol. May surface during app demo.
