## Why

`kenn-dotnet` walks every project in a `.sln` sequentially. On a large
multi-project workspace (app: 4,235 unique source files across ~50
projects in the largest .sln) this dominates wall-clock time — ~75s for
that single .sln, ~150s aggregate over three nested .slns. Every active
core but one sits idle while Roslyn semantic analysis grinds through
one project at a time.

Roslyn supports concurrent semantic queries on different `Compilation`
instances — there is no inherent reason to walk projects serially.

## What Changes

- The producer SHALL walk projects within a single `kenn-dotnet`
  invocation **concurrently**. After `MSBuildWorkspace` has finished
  loading the solution (which remains serial — `OpenSolutionAsync` is
  not concurrent-safe), the per-project walks dispatch to a bounded
  worker pool sized to `Environment.ProcessorCount`.
- All shared producer state in `IndexerCore` becomes thread-safe:
  `IdRegistry`, `FileTracker`, `JsonlSink`, edge-dedup sets,
  per-symbol-walk counters. The `_currentPackageId` field becomes a
  per-task parameter.
- The wire-format ordering invariants are preserved: every `Ref` is
  still emitted (as `PackageFrame`, `FileFrame`, `StubFrame`, or
  `SymbolFrame`) before any frame that references it. Frames from
  different projects MAY interleave at the line level, but per-project
  ordering remains coherent.
- `--max-parallelism` CLI flag added (default
  `Environment.ProcessorCount`, `1` for the previous serial behaviour
  if a user wants it back).

## Capabilities

### Modified Capabilities

- `dotnet-stream-indexer`: spec gains a "frames from concurrent walks
  may interleave" requirement and a per-emitter ordering requirement
  that replaces the previous implicit "one walk in flight" assumption.
  The wire format and frame shapes do not change.

## Impact

- **Code:**
  - `indexers/kenn-dotnet/src/Indexing/IndexerCore.cs` — replace the
    sequential per-project loop with a bounded `Parallel.ForEachAsync`,
    drop `_currentPackageId` field, add per-task parameters.
  - `indexers/kenn-dotnet/src/Indexing/IdRegistry.cs` — guard the four
    intern dictionaries with a single `lock` (contention is dominated
    by short critical sections; read-heavy traffic doesn't justify a
    `ReaderWriterLockSlim` here).
  - `indexers/kenn-dotnet/src/Indexing/FileTracker.cs` — same lock
    pattern; the file-id allocation and `_seen` map need protection.
  - `indexers/kenn-dotnet/src/Wire/JsonlSink.cs` — `lock` around
    `Write` so concurrent emitters don't interleave bytes within a
    single JSON line.
  - `IndexerCore._emittedStructuralEdges` becomes
    `ConcurrentDictionary<…, byte>` (used as a thread-safe set);
    `_emittedBodyEdges` stays per-tree (already per-walk) but the
    walker itself becomes per-task.
  - `IndexerCore._keyBuf` (single shared `StringBuilder`) becomes
    `ThreadLocal<StringBuilder>` so each worker has its own scratch
    buffer with no contention.
  - `_symbolFullCount` / `_edges` / `_errors` switch to
    `Interlocked.Increment(ref)` over `long`.
  - `indexers/kenn-dotnet/src/Cli/IndexCommand.cs` and
    `IndexOptions.cs` — new `--max-parallelism` flag.
- **APIs:** none; CLI gains one optional flag.
- **Schema:** no change.
- **Performance:** target 3-5× wall-clock improvement on the largest
  `.sln` of a multi-project C# workspace (8-core baseline). The
  `MSBuildWorkspace` solution-load phase remains serial and is the
  irreducible floor.
