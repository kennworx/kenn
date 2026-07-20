## Why

The current C# indexing path runs `scip-dotnet`, writes a full SCIP protobuf
file to disk, then reads it back and transforms it into our records. On the
app run that's hundreds of megabytes written and re-read sequentially, and
the SCIP intermediate is also where two of our worst data-loss bugs live:
`SymbolRecord.def_range` is always `[0,0,0,0]` (B2) and `packages: []` for
every C# workspace (B3). A from-scratch Roslyn-driven indexer that streams
JSONL straight to the consumer fixes both bugs by construction and removes
the disk round-trip.

## What Changes

- **NEW** Standalone .NET 10 CLI at `scratch/dotnet-stream/` that walks
  Roslyn directly (no SCIP, no protobuf) and emits envelope-tagged JSONL on
  stdout: `meta`, `file`, `symbol`, `partial_def`, `edge`, `end`. Stable
  string `pub_id`s are the only cross-reference identifier on the wire.
- **NEW** Stdin-JSONL ingest mode in `scratch/surreal-spike/` that
  resolves `pub_id` → `short_id` online (forward references stub on first
  sight, patch when the real symbol arrives), batches 10k records, and
  writes to embedded SurrealDB.
- **B2 fix by construction**: every `symbol` frame carries `def_range` taken
  directly from `ISymbol.Locations[0].GetMappedLineSpan()`. No
  occurrence-matching pass.
- **B3 fix by construction**: a synthetic root package symbol per assembly
  (`pub_id: "cs:pkg/<AssemblyName>"`, `kind: package`, `enclosing_symbol: 0`)
  is emitted, with each top-level namespace's `defined_in` edge pointing at
  it. `distinct_packages` works without a query change.
- **Edge coverage v1 (narrow)**: `defined_in`, `contains`, `implements`,
  `overrides`, `calls`. Then expand in this same change to app parity:
  `type_use`, `field_access` (with `FieldOp` read/write), `instantiates`,
  `generic_constraint`, `imports`, `corresponds_to`.
- **Locals are not symbols**. We walk into method/lambda bodies to capture
  call/reference edges (the source of every such edge is the enclosing
  fn/method/class), but local variables, loop variables, and lambda
  parameters never become symbol records. Semantic navigation, not
  source-code details.
- **B1 tokenizer probe (data-collection only)**: hand-crafted SurrealDB
  queries against a real snapshot to characterize what the `class` tokenizer
  + `@0@` operator actually do (`WebhookHandler` vs `Webhook`, lowercased
  variants, `@1@`, `@@`, manual class-tokenizer split). Output: a short
  findings file at `scratch/b1-tokenizer-findings.md`. The actual B1 fix is
  a separate proposal, written after this probe lands.
- **Out of scope**: promoting from `scratch/` to `crates/`, wiring into
  `code-intel-indexer::CSharpScipDotnet`, the actual B1 fix, and other
  languages (TS/Rust/Go/Python still go through the existing scip-* drivers).

## Capabilities

### New Capabilities
- `dotnet-stream-indexer`: Roslyn-driven C# indexer that emits a streaming
  JSONL wire format consumed by a Rust ingest prototype, fixing B2/B3 by
  construction. Includes a SurrealDB tokenizer probe whose findings inform a
  separate B1 fix proposal.

### Modified Capabilities
<!-- None. The existing scip-indexing-pipeline / source-data-model /
indexed-store-and-lifecycle / mcp-server changes are not yet archived to
specs/, and this prototype lives outside the production pipeline. The
"streaming indexer" produces the same record shapes already defined in
crates/source-model, so no spec-level changes are required there. -->

## Impact

- **New code**: `scratch/dotnet-stream/` (.NET 10 console app) and a new
  ingest subcommand in `scratch/surreal-spike/` (Rust).
- **No production code touched.** `crates/code-intel-*` are untouched in
  this change. Promotion is a follow-up after the prototype proves out on a
  real workspace (app).
- **External tooling**: requires .NET 10 SDK on PATH for anyone running the
  prototype. `MSBuildLocator.RegisterDefaults()` discovers the SDK; no
  vendored MSBuild.
- **No schema migration**: the prototype writes into a fresh SurrealDB
  database under `scratch/surreal-spike/db-*/`, reusing the existing
  `code-intel-store` schema shape conceptually but not sharing on-disk state
  with production snapshots.
- **Reference material** (read-only): `scratch/scip-dotnet/ScipDocumentIndexer.cs`
  and `ScipProjectIndexer.cs` are kept as Roslyn-usage references only; we
  do not extend them.
- **Findings artifact**: `scratch/b1-tokenizer-findings.md` becomes input to
  a separate B1 change proposal.
