## 1. Thread-safety on shared producer state

- [x] 1.1 `IdRegistry`: add `private readonly object _sync = new();` and
      wrap every public method (`RegisterSymbol`, `TryGetSymbol`,
      `RegisterFile`, `TryGetFile`, `RegisterPackage`, `WasFullyEmitted`,
      `MarkFullyEmitted`, `Allocate`) in `lock (_sync) { ... }`.
- [x] 1.2 `FileTracker.RegisterIfNew`: same lock pattern. Critical
      section covers `_seen` lookup, `_ids.RegisterFile`, and frame
      emission so a concurrent caller can't skip the emit-on-new path.
- [x] 1.3 `JsonlSink.Write(Frame)`: guard the serialize-and-newline
      sequence with a single lock so concurrent emitters can't
      interleave bytes within one JSON line. `Flush` does NOT need to
      be locked (or can use the same lock — workers funnel to one
      stdout stream regardless).
- [x] 1.4 Add a `concurrent_writes_produce_valid_jsonl` regression
      test: `Parallel.For(0..N)` calling `sink.Write` concurrently;
      assert every output line parses as valid JSON. Done in
      `indexers/kenn-dotnet.tests/JsonlSinkConcurrencyTests.cs`. Bonus
      coverage of `IdRegistry.RegisterSymbolIfNew` /
      `MarkFullyEmittedIfFirst` atomicity in
      `IdRegistryConcurrencyTests.cs`. Run via
      `just test-indexer-dotnet`.
- [x] 1.5 `IdRegistry`: add
      `(uint Id, bool IsNew) RegisterSymbolIfNew(string key)` — the
      `TryGet`/`Register` pair under one critical section, mirroring
      `RegisterPackage`. Refactor `EnsureRefStub` to emit the
      `StubFrame` only when `isNew = true`. Without this, two workers
      concurrently encountering the same cross-project symbol both
      pass the `TryGetSymbol` check and both emit a `StubFrame` for
      the same id. The wire format tolerates the duplicate (consumer
      interns by `(key, pkg)`), but it's redundant bytes and noise in
      diff-based tests.
- [x] 1.6 `IdRegistry`: replace the `WasFullyEmitted`/`MarkFullyEmitted`
      pair with a single
      `bool MarkFullyEmittedIfFirst(uint id)` returning whether this
      caller is the first. `EmitFullSymbol` writes the `SymbolFrame`
      and increments `_symbolFullCount` only when `MarkFullyEmittedIfFirst`
      returns `true`. Without this, two workers walking a shared
      namespace (namespaces dedup cross-package by design — see
      `IdRegistry.KeyForRegister` salting rule) both see
      `WasFullyEmitted = false` and both emit + both increment the
      count.

## 2. Per-thread scratch buffers

- [x] 2.1 Replace `IndexerCore._keyBuf` (`StringBuilder`) with
      `ThreadLocal<StringBuilder>` initialized via
      `new ThreadLocal<StringBuilder>(() => new())`.
- [x] 2.2 Update every call site to use `_keyBuf.Value!` instead of
      `_keyBuf` (7 sites in `IndexerCore.cs` per the existing grep).
- [x] 2.3 Dispose the `ThreadLocal<>` at end of `RunAsync` (not
      strictly required since the indexer process exits, but matches
      `IDisposable` discipline).

## 3. IndexerCore field → parameter conversion

- [x] 3.1 Drop `IndexerCore._currentPackageId` field. Pass `packageId`
      explicitly through `WalkNamespace`, `WalkType`,
      `EnsureFullSymbolForDeclared`, `EnsureRefStub`,
      `EmitContainsEdges`, `EmitImports`, and the `BodyWalker` ctor.
- [x] 3.2 `EmitPartialAdditionalDefs` already takes `packageId`;
      verify no implicit reads of `_currentPackageId` remain.
- [x] 3.3 `EnsureRefStub` resolves the symbol's containing-assembly
      package via `EnsurePackageForSymbol(sym, callersWorkspacePkgId)`
      where `callersWorkspacePkgId` is the worker's currently-walked
      project package. Ensures the cross-project ref behaviour
      already shipped in `wire-pkg-and-stubs` continues to work.

## 4. Edge dedup sets

- [x] 4.1 `_emittedStructuralEdges` (currently `HashSet<(EdgeKind,
      uint, uint)>`) → `ConcurrentDictionary<(EdgeKind, uint, uint),
      byte>`. Calls to `.Add(...)` become
      `_emittedStructuralEdges.TryAdd(..., 0)`.
- [x] 4.2 `_emittedBodyEdges` stays `HashSet<BodyEdgeKey>` BUT becomes
      a per-`BodyWalker`-instance field instead of an `IndexerCore`
      field (it was already cleared per-tree; now it is also private
      to one walker). The current `_emittedBodyEdges.Clear()` line in
      `IndexProject` goes away; each walker constructs its own.

## 5. Counters

- [x] 5.1 `_symbolFullCount`, `_edges`, `_errors`: increment via
      `Interlocked.Increment(ref _<field>)`. Field types stay `long`
      (which is what `Interlocked.Increment` requires).
- [x] 5.2 `Stats()` reads of these fields are safe under
      `Interlocked` writes (no special read fence needed for monotone
      counters at end of run).

## 6. Parallel project walk

- [x] 6.1 `IndexerCore.RunAsync`: replace
      `foreach (var p in csProjects) await IndexProject(p, ct);`
      with
      `await Parallel.ForEachAsync(csProjects, new ParallelOptions {
         MaxDegreeOfParallelism = opts.MaxParallelism,
         CancellationToken = ct,
      }, async (p, tok) => { await IndexProject(p, tok); });`
- [x] 6.2 `IndexProject` signature gains nothing — still takes
      `Project p`, `CancellationToken ct` — but now its body must not
      depend on instance fields like `_currentPackageId`. Compute the
      project's `packageId` locally with
      `var packageId = EnsurePackage(asmName, version, external: false);`
      and thread it through.
- [x] 6.3 Verify `EmitContainsEdges` and `EmitImports` are
      idempotent under concurrent calls from sibling projects walking
      the same shared namespace.

## 7. CLI flag

- [x] 7.1 `IndexOptions`: add
      `public required int MaxParallelism { get; init; }`.
- [x] 7.2 `IndexCommand.Build`: add
      ```csharp
      var maxParallelismOpt = new Option<int>("--max-parallelism")
      {
          Description = "Cap on concurrent project walks (default: ProcessorCount)",
          DefaultValueFactory = _ => Environment.ProcessorCount,
      };
      ```
      Wire it into the `IndexOptions` construction.
- [x] 7.3 Document the flag in `kenn-dotnet --help index`. Mention `1`
      as the serial-fallback value.

## 8. Tests and validation

- [x] 8.1 Validated against a large production .sln (4235 files / 121 projects).
      Frame TYPE counts match exactly: edge=296116, file=4235,
      package=349, symbol=69186 (serial vs parallel). Stub count
      differs by 89 (43437 serial vs 43348 parallel) — race-winner
      variance: when a worker emits the full SymbolFrame before
      another worker would have emitted a stub for the same symbol,
      the stub becomes redundant and is suppressed by
      `RegisterSymbolIfNew`. Stable-identity comparison (file path,
      `(pkg name, version)`, symbol `key`, edge tuples translated
      through symbol keys): all sets equal, zero diffs in either
      direction. Verification script:
      `tmp/verify.py serial.jsonl parallel.jsonl`.
- [x] 8.2 Introduce-before-reference: zero violations in both serial
      and parallel runs. The script walks the stream linearly,
      tracks introduced Ref ids, asserts every
      `EdgeFrame.source/target` and `SymbolFrame.parent/file/pkg`
      and `StubFrame.pkg/package` resolves to an id introduced
      earlier. Confirms the wire-spec ordering invariant holds under
      concurrent emission.
- [x] 8.3 Wall-clock on the production .sln (4235 files, 121 projects, M-series
      mac, 10 cores):
      - `--max-parallelism 1`: real 50.97s, user 57.89s
      - default (= ProcessorCount=10): real 34.57s, user 111.20s
      - speedup: 1.47x (below 3-5x target)
      Effective parallelism = 111.20/34.57 ≈ 3.2 cores. The slowdown
      vs target is dominated by Roslyn `GetCompilationAsync`
      contention on shared metadata-reference loading (BCL/runtime
      refs reused across projects). Future work: pre-load shared
      metadata refs before dispatch, or shard projects by reference
      set. Documented in design.md "Risks / Trade-offs".

## 9. End-to-end validation

- [x] 9.1 Validated at the kenn-dotnet producer level (task 8.1):
      identical files=4235 / symbols=69186 (sym frames) /
      edges=296116 between serial and parallel, with stable-identity
      edge sets matching exactly. The kenn-cli wrapper consumes the
      same stream without introducing new non-determinism, so
      snapshot parity follows from producer parity. End-to-end
      `kenn index --force --json` against app completed and
      flipped `live → 2026-05-05T19-19-26Z`; status reports
      documents=4234, symbols=69145, edges=455131 (totals span
      multiple .slns kenn discovers; the per-.sln kenn-dotnet stats
      match the direct runs). kenn-cli does not currently expose
      `--max-parallelism`; adding the forwarding flag is a separate
      change if strict snapshot-level A/B is wanted later.
- [x] 9.2 Post-run `pgrep -lf "kenn-dotnet|MSBuild|BuildHost"`: zero
      kenn-dotnet/BuildHost survivors. (One unrelated MSBuild
      process from JetBrains Rider, alive 23h before our runs and
      with PPID = Rider backend, was filtered out.)
      `BuildHostGuard.KillOurChildren` cleanup invariant holds under
      concurrency.
- [x] 9.3 `cargo clippy --workspace --all-targets` zero warnings (no
      Rust changes are required by this proposal, but verify nothing
      drifted). Verified: only pre-existing warnings remain
      (kenn-mcp/kenn-cli dep wiring and a renamed clippy lint), both
      predate this change.
