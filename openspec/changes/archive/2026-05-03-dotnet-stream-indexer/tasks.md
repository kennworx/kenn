## 1. Producer scaffold (.NET 10 console app)

- [x] 1.1 Create `scratch/dotnet-stream/` with `global.json` pinning SDK 10.0.0 (rollForward latestMajor, allowPrerelease false), a `.csproj` targeting `net10.0`, and an empty `Program.cs`
- [x] 1.2 Add NuGet refs: `Microsoft.Build.Locator`, `Microsoft.CodeAnalysis.CSharp.Workspaces`, `Microsoft.CodeAnalysis.Workspaces.MSBuild`, `Microsoft.Extensions.FileSystemGlobbing`, `System.CommandLine` (matching scip-dotnet versions for stability)
- [x] 1.3 Wire `MSBuildLocator.RegisterDefaults()` before any MSBuild types load (Program entrypoint)
- [x] 1.4 Define `System.CommandLine` root command with the producer CLI flags from spec §"Producer CLI Flags" (`--workspace`, `--projects`, `--include`, `--exclude`, `--skip-restore`, `--restore-timeout-ms`, `--flush-bytes`, `--flush-frames`, `--edge-kinds`, `--output`, `--log-level`)
- [x] 1.5 Set up stderr-only logger via `Microsoft.Extensions.Logging` honoring `--log-level`; assert nothing log-shaped is ever written to stdout
- [x] 1.6 Smoke test: `dotnet run --project scratch/dotnet-stream -- --help` prints all flags and exits 0

## 2. Producer wire format & batching

- [x] 2.1 Define C# DTOs for each frame kind (`MetaFrame`, `FileFrame`, `SymbolFrame`, `PartialDefFrame`, `EdgeFrame`, `EndFrame`) with `[JsonPropertyName("type")]` discriminator and snake_case field names matching the spec examples
- [x] 2.2 Implement `JsonlSink` that serializes each frame with `System.Text.Json` (omit-null defaults), appends `\n`, and writes to an internal `BufferedStream` over `Console.OpenStandardOutput()`
- [x] 2.3 Implement batched flush: track buffered byte count and frame count; flush when either threshold from `--flush-bytes`/`--flush-frames` is hit
- [x] 2.4 Flush on `EmitEnd()` and register a `ProcessExit` handler that flushes on shutdown including non-zero exit
- [x] 2.5 Wire optional `--output <file>` tee: every flush writes the same bytes to the file as well as stdout
- [x] 2.6 ~~Unit test: emit 10000 mixed frames…~~ — skipped per user direction; e2e on eShopOnWeb in §7 supersedes this

## 3. Producer Roslyn walk — symbols and basic edges

- [x] 3.1 Lift `ScipProjectIndexer` skeleton from `scratch/scip-dotnet/`: open solution / project, dedupe target frameworks (prefer `(net10.0)`), filter by `--include`/`--exclude`
- [x] 3.2 Implement `dotnet restore` invocation with `--skip-restore` / `--restore-timeout-ms` honoring the flags; log result on stderr
- [x] 3.3 Implement `IsLocalSymbol` predicate (port from `ScipDocumentIndexer.cs`) — gates every emission
- [x] 3.4 Implement `pub_id` builder per scheme in design D2 with overload disambiguation by parameter signature; assert two `Bar()` / `Bar(int)` get distinct ids in a unit test (verified e2e: scip-dotnet has overloaded methods that get distinct pub_ids)
- [x] 3.5 Walk every project's `Compilation`: for each `INamedTypeSymbol` and every `ISymbol` declared in source that is not local, emit one `SymbolFrame` with `def_range` from `Locations[0].GetMappedLineSpan()`, `signature_doc` and `documentation` populated when present (omitted when both empty)
- [x] 3.6 Emit one `FileFrame` on first sight of each source file with `path` (workspace-relative), `is_test` (`true` if path matches `*.Tests.*`/`tests/`/`Tests/` heuristic; revisit later), `is_external: false`, `content_hash` as 16-hex-char xxh64 of UTF-8 bytes
- [x] 3.7 For every type-kind symbol, emit `defined_in` edge to its containing namespace/type; for every namespace, emit `defined_in` to its parent or to the synthetic root package
- [x] 3.8 Emit synthetic root package per assembly: one `SymbolFrame` with `kind: package`, `pub_id: cs:pkg/<AssemblyName>`, and `defined_in` edges from every top-level namespace to it
- [x] 3.9 Emit `contains` edges from each module/namespace symbol to every file that contributes a top-level declaration to it
- [x] 3.10 Smoke run on `scratch/scip-dotnet` (used in lieu of eShopOnWeb for fast iteration): `meta` + `end` + 19 files + 811 symbols + 1964 edges (`calls`/`contains`/`defined_in`/`implements`/`overrides` all present)

## 4. Producer Roslyn walk — edges v1 narrow

- [x] 4.1 Walk inside method/constructor/accessor/lambda bodies; for every `InvocationExpressionSyntax`, resolve `SymbolInfo`; emit `calls` edge with `source_pub_id` = enclosing fn/method/class, `target_pub_id` = called symbol, `range` = invocation span. Skip locals as both source and target
- [x] 4.2 For every `INamedTypeSymbol`'s `BaseType` chain (excluding `System.Object`/`Enum`/`ValueType`), emit `implements` edge from the type to each base
- [x] 4.3 For every `INamedTypeSymbol`'s `AllInterfaces`, emit `implements` edge from the type to each interface
- [x] 4.4 For every `IMethodSymbol`'s `OverriddenMethod` chain, emit `overrides` edge; for every `InterfaceImplementations(method)` (port the helper from scip-dotnet), emit `overrides` edge
- [x] 4.5 Smoke run: confirmed `calls=849, contains=31, defined_in=810, implements=96, overrides=178` on scip-dotnet workspace

## 5. Producer Roslyn walk — edges expansion to app parity

All 11 edge kinds verified on app DB.

- [x] 5.1 `type_use` from `IdentifierNameSyntax` / `GenericNameSyntax` resolving to `INamedTypeSymbol` (app: 91,753)
- [x] 5.2 `field_access` with `field_op: read|write` classified by parent context (assignment LHS, ref/out arg, ++/--) (app: 229,667)
- [x] 5.3 `instantiates` from generic type arguments at construction + identifier sites (app: 15,083)
- [x] 5.4 `generic_constraint` from `ITypeParameterSymbol.ConstraintTypes` on both type and method parameters (app: 229)
- [x] 5.5 `imports` from `UsingDirectiveSyntax`, source = enclosing namespace symbol or root package (app: 4,606)
- [x] 5.6 `corresponds_to` + `partial_def` frames for partial classes via `DeclaringSyntaxReferences` (app: 9)
- [x] 5.7 `--edge-kinds` allowlist honored — IndexerCore filters at emission via `EdgeKindAllowlist`

## 6. Consumer ingest mode in surreal-spike

- [x] 6.1 Add `ingest-jsonl` subcommand to surreal-spike's `main`, with manual arg parsing matching the existing style (see `print_help` / `parse_iter`)
- [x] 6.2 Wire CLI flags from spec §"Consumer CLI Flags": `--db` (required), `--batch-size` (default 10000), `--reset-db`, `--input` (default stdin), `--quiet`, `--progress`
- [x] 6.3 Define Rust `Frame` enum with `#[serde(tag = "type", rename_all = "snake_case")]` and per-variant payload structs mirroring the C# DTOs
- [x] 6.4 Read input line-by-line via `BufReader`; deserialize each line; on parse error, log the line number and continue (don't abort)
- [x] 6.5 Implement `IdRegistry`-style resolver: `pub_id` → `short_id`. Unknown pub_id from an edge registers a stub `is_external: true`/`def_range: [0,0,0,0]`; real `symbol` frame patches the row via UPSERT keyed by pub_id
- [x] 6.6 Accumulate `FileRecord` / `SymbolRecord` / `SymbolDocsRecord` / `EdgeRecord` (PartialDef ignored in v1); flush at `--batch-size`
- [x] 6.7 Flush remaining records on `end` frame; print stats on stderr unless `--quiet`
- [x] 6.8 Define a fresh schema (own `symbol`/`file`/`symbol_docs` tables + RELATE per edge kind) — surreal-spike's existing schema is for SCIP shape, this prototype runs alongside it. `--reset-db` removes the directory before re-running schema setup

## 7. Consumer integration

- [x] 7.1 Smoke test: ingested 2796-line JSONL produced from scip-dotnet workspace. `verify-jsonl` reports B2 PASS, B3 PASS, all v1 edges present (defined_in 810, contains 31, calls 849, implements 96, overrides 178)
- [x] 7.2 End-to-end on eShopOnWeb via `spike pipeline` (full 11-kind coverage): **7.05 s wall**. 279 files, 2291 symbols, 11382 edges. verify-jsonl: B2 PASS, B3 PASS (10 packages), all 11 edges PASS (defined_in 2292, contains 528, calls 2697, type_use 1787, field_access 2950, instantiates 439, implements 278, overrides 101, imports 296, generic_constraint 9, corresponds_to 5)

## 8. End-to-end on app

- [x] 8.1 Real piped e2e via `spike pipeline` (Rust spawns `dotnet run` and reads its stdout — no intermediate file): **114.83 s** total wall on app. eShopOnWeb same path: **7.05 s**
- [x] 8.2 Baseline was `7:46 / 224 MB / 4345 docs / 43,111 symbols / 20,088 edges`. New: `1:55 / 4790 files / 88,441 symbols / 570,165 edges (full 11-kind coverage)`. Wall time **4× faster** despite emitting ~28× more edges
- [x] 8.3 verify-jsonl PASS on db-app: 121 packages, 0 symbols with bogus def_range (excluding externals + packages), all 11 edge kinds present (defined_in 88448, contains 9114, calls 107592, type_use 91753, field_access 229667, instantiates 15083, implements 12693, overrides 10971, imports 4606, generic_constraint 229, corresponds_to 9). B2 + B3 closed by construction

## 9. B1 tokenizer probe

- [x] 9.1 Probed two snapshots: db-eshop (3,131 symbols) and db-app (98,132 symbols). Both built from this prototype's pipeline
- [x] 9.2 Ran `name @N@`, `display_name @N@`, `CONTAINS` queries with raw camelCase, lowercased, and manually class-split variants
- [x] 9.3 `name CONTAINS '<target>'` confirms ground truth: 6 hits on app for `WebhookController`, 3 on eshop for `BasketService` — the data IS present
- [x] 9.4 Wrote `scratch/b1-tokenizer-findings.md` — and the canned hypothesis was **wrong**. The same `@0@ '<CamelCase>'` query that returns 0 on app returns 5 on eShopOnWeb. The bug is scale-sensitive (or compound-token sensitive), not pure tokenizer mismatch
- [x] 9.5 Findings file now has three plausible fix directions; emphasizes that the simple "tokenize the query client-side" fix from next-task-notes.md is **not sufficient on its own** (verified: `@0@ 'webhook controller'` returns 0)

## 10. Validate and clean up

- [x] 10.1 `openspec validate dotnet-stream-indexer --strict` → "Change 'dotnet-stream-indexer' is valid"; 4/4 artifacts complete
- [x] 10.2 README at `scratch/dotnet-stream/README.md` — flags table, JSONL frame examples, pub_id scheme, perf reference
- [x] 10.3 README addition `scratch/surreal-spike/README-jsonl.md` documenting `ingest-jsonl` / `verify-jsonl` / `b1-probe` subcommands and the schema
- [x] 10.4 `dotnet build`: **0 warnings**. `cargo clippy --release` on surreal-spike: 7 warnings, all in pre-existing main.rs (not in new jsonl.rs); surreal-spike is a separate Cargo workspace not under the project's pedantic config
