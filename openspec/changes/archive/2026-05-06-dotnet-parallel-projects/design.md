## Context

`IndexerCore.RunAsync` today is a triple-nested serial loop:

```
for entry in .sln entries:                    (orchestrator-level, in Rust)
    open MSBuildWorkspace
    for project in solution:                  (← this proposal targets this)
        compilation = await GetCompilationAsync()
        WalkNamespace + WalkType (recursive)
        for tree in compilation.SyntaxTrees:
            new BodyWalker(this, model).Visit(root)
```

Empirically on a large multi-project C# workspace, the per-project
walk dominates: solution loading is ~5-10s, the per-project walks
sum to ~70s. Every CPU core except one sits idle while Roslyn does
semantic analysis on one project at a time.

Roslyn explicitly supports concurrent access to different
`Compilation`/`SemanticModel` instances — see the type's documented
thread-safety contract. The blocker is not Roslyn; it is the shared
mutable state in our own `IndexerCore`.

## Goals / Non-Goals

**Goals.**

- Walk projects within a single `kenn-dotnet` invocation in parallel
  on a worker pool sized to `Environment.ProcessorCount`.
- Preserve every wire-format invariant the consumer relies on
  (id-before-reference, exactly-one `meta`, exactly-one `end`, frame
  per-line atomicity).
- No change to the `JSONL` shape or to the consumer pipeline. The
  Rust ingest path consumes the same stream.

**Non-Goals.**

- Per-syntax-tree parallelism inside a project. Once project-level
  parallelism saturates the cores, finer-grained parallelism adds
  contention without measurable wall-clock benefit.
- Per-`.sln` parallelism in the Rust orchestrator. Useful when slns
  are independent; on the app shape (nested slns) it gives no win.
  Orthogonal change, separately specifiable.
- `MSBuildWorkspace.OpenSolutionAsync` parallelism. Documented as not
  thread-safe; we keep solution loading serial.

## Decisions

### Decision 1: Worker pool via `Parallel.ForEachAsync`

Use `Parallel.ForEachAsync(csProjects, options, IndexProjectAsync)`
with `MaxDegreeOfParallelism = opts.MaxParallelism`. Default to
`Environment.ProcessorCount`; allow `--max-parallelism 1` for the
sequential fallback (debug, deterministic frame ordering, etc.).

Compilations are loaded **inside** each worker via
`project.GetCompilationAsync(ct)`. Roslyn's `Compilation` is lazily
materialized and cached on the `Project` instance after first await,
so even if two workers happen to pick up the same project (they
won't — we partition `csProjects`), the second await returns the
cached instance.

The empirically-correct order is: `MSBuildWorkspace.LoadProjectsAsync`
serially → partition into `csProjects` → `Parallel.ForEachAsync` → each
worker calls `GetCompilationAsync` and walks. The solution-load phase
remains the irreducible serial floor.

### Decision 2: Lock granularity for `IdRegistry`

Three intern dictionaries (`_symbolKey`, `_filePath`, `_packageKey`)
plus `_fullyEmitted`. All operations are short — dictionary lookups,
inserts, hash-set adds. Critical sections are sub-microsecond. A
single `lock (_sync)` per public method beats a
`ReaderWriterLockSlim` for this workload (writer contention is high
during early walks, then drops; read-vs-write asymmetry is small).

```csharp
private readonly object _sync = new();
public uint RegisterSymbol(string key)
{
    lock (_sync)
    {
        if (_symbolKey.TryGetValue(key, out var existing)) return existing;
        _next += 1;
        _symbolKey[key] = _next;
        return _next;
    }
}
```

`_next` (the id counter) lives inside the same lock — using
`Interlocked.Increment` on it would race with the dictionary inserts
(an id could be allocated to one key but lose its dictionary slot to
another).

### Decision 2b: Atomic `register-and-emit` for stubs and full symbols

Locking the public `IdRegistry` methods is necessary but not sufficient.
Two call sites still have a non-atomic check-then-act sequence across
the unlock boundary that the per-method lock does NOT close:

- `EnsureRefStub` does `TryGetSymbol(key) → if missing, RegisterSymbol →
  emit StubFrame`. Two workers can both miss the `TryGet`, both call
  `RegisterSymbol` (which under the lock is idempotent and returns the
  same id), and both emit a `StubFrame` for that id.
- `EmitFullSymbol` does `WasFullyEmitted(id) → write SymbolFrame → if
  first, MarkFullyEmitted + increment _symbolFullCount`. Two workers
  walking a shared namespace (namespaces are intentionally
  cross-package via the `pkgId == 0` salt rule in
  `IdRegistry.KeyForRegister`) can both see `first = true` and both
  emit + both increment.

The wire format tolerates duplicate stubs and duplicate full frames —
the consumer interns by `(key, pkg)` and the upgrade-stub-to-full
logic is order-tolerant — but `_symbolFullCount` over-counts and the
output stream gains redundant bytes.

The fix mirrors what `IdRegistry.RegisterPackage` and
`FileTracker.RegisterIfNew` already do: fold the lookup, allocation,
and "is this caller responsible for emitting" decision into one
locked operation that returns `(id, isNew)` (or `wasFirst`). The
caller then emits conditionally. This is captured in tasks 1.5 and
1.6.

### Decision 3: Per-thread `StringBuilder`

`_keyBuf` is a single `StringBuilder` reused across `PubId`/`Key`
calls. Concurrent walkers would corrupt each other's intermediate
state. Replace with `ThreadLocal<StringBuilder>` initialized lazily;
each worker thread gets its own. The buffers persist for the
lifetime of the worker thread (they are reused across many keys per
worker), so the allocation count stays at `O(threads)` not
`O(symbols)`.

The downstream `IdRegistry.Key` /`KeyForRegister` and `PubId.For*`
APIs already take a `StringBuilder` parameter — call sites just pass
`_keyBuf.Value` instead of `_keyBuf`.

### Decision 4: `JsonlSink` write atomicity

`JsonlSink.Write(Frame)` serializes one frame to a buffered
`Utf8JsonWriter` and emits a `\n`. Concurrent calls would interleave
bytes from different frames, breaking the JSONL "one frame per line"
invariant.

A single `lock` around the write path is sufficient — the
serialization itself is fast (microseconds), and downstream stdout
flushing is amortized via the existing `--flush-bytes` /
`--flush-frames` thresholds.

The lock is on `Write`, NOT on `Flush`. Workers contending on `Flush`
would funnel into a single I/O operation anyway, but the typical case
is many small `Write`s and rare `Flush`es.

### Decision 5: Ordering invariants under interleaving

The wire format requires: every `Ref` referenced by a frame's
`source`, `target`, `parent`, `pkg`, or `file` field is introduced
(at least as a `StubFrame`/`PackageFrame`/`FileFrame`) **earlier in
the stream**. Today this is guaranteed by per-walker emission order.
Under parallelism, we keep the same property by ensuring each
worker's emission order respects "introduce before reference":

- `EnsureRefStub` is called before any edge or symbol frame
  references the resulting id — same as today.
- `EnsurePackage` / `EnsurePackageForSymbol` emits the
  `PackageFrame` before any frame that uses its id — same as today.
- `_files.RegisterIfNew` emits the `FileFrame` before any frame
  referencing its id — same as today.

Within a single worker, the emission order is unchanged. **Across**
workers, frames may interleave at the line level — but every emitter
holds the `JsonlSink` lock for the duration of one frame's
serialization, so a frame-line is atomic. A reader processing the
stream sees a valid JSONL with a possibly-different frame interleave
than the serial run; the wire spec already accommodates this since
it never requires a specific cross-emitter order, only the
introduce-before-reference relation.

### Decision 6: Counters via `Interlocked`

`_symbolFullCount`, `_edges`, `_errors` are simple monotone counters.
`Interlocked.Increment(ref _symbolFullCount)` is atomic, lock-free,
and adequate. Switch the field types from `long` (already) to keep
the same name and just change the increment site.

## Risks / Trade-offs

- **Lock contention on `IdRegistry`**. Heavily-shared read-modify-write
  paths could serialize behind the lock. Measurement: profile the lock
  hold time on the largest .sln; if > 5% of wall clock, revisit with
  per-shard hash-table or `ConcurrentDictionary`.

- **Roslyn `GetCompilationAsync` contention dominates the speedup
  ceiling.** Measured on a large production .sln (4235 files, 121 projects, 10-core
  M-series mac): serial wall 50.97s vs parallel wall 34.57s = 1.47x
  speedup. User-time goes from 57.89s to 111.20s — parallel does ~2x
  more total CPU work. Effective parallelism ≈ 3.2 cores, not 10.
  Hypothesis: per-project `GetCompilationAsync` materializes shared
  metadata references (BCL, runtime, common NuGet refs) under
  Roslyn-internal locks; concurrent projects redundantly fault those
  in. Pre-warming shared metadata-reference assemblies before
  dispatch, or sharding projects by their reference set so
  cache-cold loads don't collide, would lift the ceiling. Out of
  scope for this change; recorded for future work.

- **`MSBuildWorkspace` thread-safety**. We keep the workspace itself
  on the main thread (project loading, diagnostic enumeration). Each
  worker calls `project.GetCompilationAsync(ct)` and reads
  `compilation.SyntaxTrees` / `GetSemanticModel(tree)` — those are
  Roslyn-documented as concurrent-safe per `Compilation`.

- **GC pressure under parallelism**. Per-thread `StringBuilder` and
  per-tree `BodyWalker` instances scale with thread count, not with
  symbol count. Net allocation should not increase.

- **Determinism**. Serial output is bit-stable; parallel output is
  not (frames interleave by completion order). Snapshot tests that
  pin specific JSONL line orderings would break — none exist today
  in our test suite, but worth noting.

- **`--max-parallelism 1` escape hatch** preserves the old serial
  behaviour for debugging or for users on memory-constrained
  machines where per-thread `Compilation` working sets matter.

## Migration Plan

No on-disk migration. New runs use the parallel path by default. To
roll back behaviourally: `kenn-dotnet index --max-parallelism 1`.

If lock contention turns out to be a real problem at scale, the
`IdRegistry` lock can be sharded by `key.GetHashCode() % N` later
without touching the call sites.
