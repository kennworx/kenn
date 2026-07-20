# kenn-dotnet

Streaming C# indexer. Roslyn directly → JSONL frames on stdout.

## Run

```sh
dotnet build
dotnet run --no-build -- index --workspace /path/to/csharp/repo
# JSONL on stdout; logs on stderr.
```

Common shape:

```sh
dotnet run --no-build -- index \
    --workspace /path/to/repo \
    --projects /path/to/repo/MySolution.sln \
    --skip-restore
```

## Self-contained publish

`dotnet publish -c Release -r <rid>` produces a single executable that
embeds the .NET runtime — clients do not need .NET installed to run
`kenn-dotnet`. They DO still need a .NET SDK on their machine because
`MSBuildWorkspace` discovers it at runtime to evaluate the target
workspace's `.csproj`/`.sln`; if they have C# code to index, they
already have an SDK.

```sh
dotnet publish -c Release -r osx-arm64    # ~43 MiB single file
dotnet publish -c Release -r osx-x64
dotnet publish -c Release -r linux-x64
dotnet publish -c Release -r linux-arm64
dotnet publish -c Release -r win-x64
# Output: bin/Release/net10.0/<rid>/publish/kenn-dotnet
```

Roslyn is pinned to 4.7 (last pre-`BuildHost` release). 4.8+ moved
`MSBuildWorkspace` to an out-of-process host that doesn't survive
`PublishSingleFile` extraction. Trimming is off — Roslyn + MSBuild + MEF
use reflection in ways the IL linker can't see through. Compression
keeps the binary under 50 MiB.

## Flags

| flag | default | meaning |
|---|---|---|
| `--workspace <dir>` | cwd | workspace root |
| `--projects <path>...` | (discover under workspace) | explicit `.sln`/`.csproj` list |
| `--include <glob>...` | `**` | file-path include filters |
| `--exclude <glob>...` | (none) | file-path exclude filters |
| `--skip-restore` | false | skip `dotnet restore` before indexing |
| `--restore-timeout-ms <int>` | 300000 | timeout for `dotnet restore` |
| `--flush-bytes <int>` | 1 048 576 (1 MiB) | flush stdout when buffered bytes exceed |
| `--flush-frames <int>` | 4096 | flush stdout when buffered frame count exceeds |
| `--edge-kinds <csv>` | (all) | restrict emitted edges to these kinds |

Log level is controlled by `KENN_DOTNET_LOG` (`Trace`, `Debug`, `Information`,
`Warning`, `Error`; default `Information`). stderr is the only logging channel;
stdout is reserved for JSONL. The producer batches output (default 1 MiB / 4096
frames) rather than flushing per line.

`obj/` and `bin/` are excluded by default — generated source under those dirs
(Razor `*.g.cs`, etc.) changes every build and shouldn't enter the index.

## JSONL frame types

Cross-references on the wire are numeric `Ref`s (u32 ids) assigned by the
producer at first sight. Files and symbols share a single id space; ids are
NOT stable across runs. See [`indexers/frames.ts`](../frames.ts) for the
canonical schema.

```jsonl
{"type":"meta","v":1,"project_root":"file:///abs","tool":"kenn-dotnet","tool_version":"0.1.0","language":"csharp"}
{"type":"file","id":1,"path":"src/Foo.cs","is_test":false,"is_external":false,"content_hash":"a1b2c3d4e5f60718"}
{"type":"symbol","id":2,"kind":"class","name":"Foo","display_name":"class Foo","parent":3,"file":1,"def_range":[10,13,10,16],"is_partial":false,"args_arity":0,"generic_arity":0,"is_external":false,"is_test":false}
{"type":"partial_def","symbol":2,"file":4,"range":[1,0,40,1]}
{"type":"edge","edge_kind":"defined_in","source":5,"target":2}
{"type":"edge","edge_kind":"calls","source":5,"target":6,"range":[12,8,12,16]}
{"type":"end","stats":{"files":254,"symbols":2288,"edges":11192,"errors":0}}
```

## Edge coverage

`defined_in`, `contains`, `calls`, `implements`, `overrides`, `type_use`,
`field_access` (with `field_op`), `instantiates`, `generic_constraint`,
`imports`, `corresponds_to`.

## Layout

```
src/Program.cs                 CLI bootstrap, MSBuildLocator, command-line parsing
src/Cli/IndexOptions.cs        options DTO
src/Cli/IndexCommand.cs        handler that drives IndexerCore
src/Wire/Frames.cs             JSONL frame DTOs (manual Utf8JsonWriter, trim-safe)
src/Wire/JsonlSink.cs          buffered + batched writer to stdout
src/Indexing/SymbolFilter.cs   IsLocalSymbol / IsInSource
src/Indexing/PubId.cs          producer-internal stable string keys (never on wire)
src/Indexing/IdRegistry.cs     pub_id → numeric Ref allocator + stub-then-upgrade tracking
src/Indexing/KindMap.cs        SymbolKind/TypeKind → "class"/"method"/etc.
src/Indexing/RangeUtil.cs      Location → [sl,sc,el,ec]
src/Indexing/SolutionLoader.cs .sln/.csproj open + restore + TFM dedupe
src/Indexing/FileTracker.cs    FileFrame emission, xxh64 hex content hash
src/Indexing/IndexerCore.cs    main walker (namespaces, types, members, edges)
```
