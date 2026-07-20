## Context

The **logical data model** — public symbol-ID format, `Kind` enum, `EdgeKind` enum, table/relation schema, wire location format — is defined in the `source-data-model` proposal. This proposal is the **producer** that emits records conforming to that contract. Anything in this design about the *shape* of records is a write-side view of `source-data-model`; if the schema changes, that proposal changes, and this one follows.

The MCP code structure server needs accurate symbol/reference/dep data over multi-language monorepos. A spike with `scip-dotnet` on real workspaces (a small C# sample (~10k LoC), a 303k-LoC C# repo) showed:

- Cold-but-cached run: ~300 µs/LoC, ~220 bytes/LoC of source. 1M LoC ≈ 5 min, ~220 MB.
- SCIP relationships give us inheritance/implements directly. Has-a/uses-a edges derive from `(occurrence ∩ enclosing-symbol declaration range)`.
- Symbol ids are stable for the same source file across indexer runs, but **not unique** across projects with the same root namespace (scip-dotnet uses a placeholder `nuget . .` package descriptor for local code). Dedup must key on `(canonical_path, symbol, range)`.
- Multi-`.sln` workspaces are common; each indexer reports paths relative to its own `metadata.project_root`. Merging requires re-rooting to the workspace.
- Real codebases trigger graceful per-project failures (NuGet vulnerability errors, missing `.csproj` references, MSBuild SDK mismatches). The pipeline must tolerate these.
- Linked git worktrees may live anywhere on disk (not only `.worktrees/`); when one is a descendant of the workspace root, the indexer must consult `git worktree list` to find and exclude it.
- **SCIP indexer field coverage is uneven and not predictable from "SCIP supports field X".** A per-language probe (scip-dotnet on a 303k-LoC C# repo C#; scip-typescript on some-org/frontend-shared; scip-python on pallets/click; scip-go on julienschmidt/httprouter; rust-analyzer on BurntSushi/byteorder) shows:

  | Field | scip-dotnet | scip-typescript | scip-python | scip-go | rust-analyzer |
  |---|---|---|---|---|---|
  | `Occurrence.enclosing_range` (on container-defs) | **0%** | ~12% | ~26% | ~12% | **0%** |
  | `SymbolInformation.kind` (non-zero) | **0%** | **0%** | **0%** | 99.6% | 100% |
  | `SymbolInformation.display_name` | (untested) | **0%** | **0%** | 100% | 99.8% |
  | `SymbolInformation.signature_documentation` | (untested) | **0%** | **0%** | 100% | 100% |
  | `SymbolInformation.relationships` (count) | 12,405 | 2 | 152 | 10 | **0** |

  TS/Py/Go ratios on `enclosing_range` reflect that only container defs (functions/classes/methods) carry it — that's correct semantics, not a gap; the absolute coverage is what matters. C# and Rust populate it on **no** defs. `Kind` and `display_name` are present in scip-go and rust-analyzer, absent in C#/TS/Python. Compensation must be per-indexer.

  **Container-coverage check (does every "ref inside a function/method" land in some container?):**

  | Indexer | refs | refs inside a container | hit% |
  |---|---|---|---|
  | scip-typescript | 867 | 867 | 100% |
  | scip-python | 22,620 | 20,929 | 92% |
  | scip-go | 3,458 | 3,323 | 96% |

  The 4-8% miss for Python/Go is exclusively file-scope refs (top-of-file imports, module-level statements in `conf.py` and example scripts) — refs that aren't "from a function/method" by definition. Tier 3 fallback for these is the SCIP-emitted module symbol (the trailing-`/` symbol). For TS/Py/Go we need only tier 1 (SCIP) + a module-symbol fallback for file-scope refs.

  **rust-analyzer encodes trait-impl relationships as impl-block symbols, not Relationships.** Verified on quinn (10k symbols, 60k occs): zero `Relationships` records, but trait-impls appear as a structural symbol pattern: `mod/impl#[TYPE][TRAIT]` for the impl block itself, plus `mod/impl#[TYPE][TRAIT]method().` for each impl-block method. This is a different encoding than scip-typescript's relationship records — richer (the impl block is its own queryable symbol) but requires a Rust-specific adapter to emit canonical `implements` / `override` edges from the symbol grammar.

This design covers only the **producer side**: orchestrating SCIP indexers and producing a normalized intermediate representation. DB choice and bulk-ingest performance are out of scope (separate proposal).

## Goals / Non-Goals

**Goals:**

- A Rust crate that, given a workspace root, runs the configured SCIP indexers out-of-band and emits a streaming sequence of normalized records (symbols, occurrences, edges) plus a structured run report
- A data model usable by SCIP indexers today. Where the indexer emits `Occurrence.enclosing_range` empty (scip-dotnet, rust-analyzer), a small **language-specific positional refinement** reads the source file as text and corrects systematic FROM-attribution errors of the bare last-preceding-def heuristic. No AST parser. Verified per-indexer: needed for scip-dotnet and rust-analyzer; not needed for scip-typescript, scip-python, scip-go.
- A symbol-kind classifier that derives kind from the SCIP symbol-string descriptor grammar when `SymbolInformation.kind` is unset. Verified-empty for scip-dotnet, scip-typescript, scip-python; native for scip-go, rust-analyzer.
- Multi-`.sln` / multi-language merge with provenance preserved
- Path canonicalization, worktree exclusion, per-project failure tolerance, opt-in C# `Directory.Build.props` provisioning
- Streaming protobuf parse so a single 4 MB document does not balloon memory
- Empirical anchors maintained: ~5 min for 1M LoC, ~220 MB output

**Non-Goals:**

- Embedded DB choice or schema mapping (next proposal). The producer emits records into a streaming `Sink` trait; the DB-ingest layer implements it.
- MCP tool surface. The producer is queried only by the eventual ingest layer.
- Tree-sitter Tier 1. Same data model, separate producer, separate proposal.
- LSP integration / live-query path. Not in scope ever for this proposal.
- Incremental indexing (file-watcher → reindex one file). Day-1 is full-unit reindex; per-file diffs become a follow-up once we measure where it matters.
- "Universal SCIP indexer" CLI. We wrap whatever each language's existing SCIP binary does; we do not write new indexers.

## Decisions

### D1. Rust crate structure: `kenn-indexer`

Single library crate exposing two main types:
- `IndexerDriver`: owns config, registry of language indexers, workspace root, exclusion rules. Methods: `discover_units`, `run_unit`, `run_all` (parallel).
- `IntermediateRepr`: the normalized data model types (`Symbol`, `Occurrence`, `Edge`, `RunReport`).

The crate has a `Sink` trait the consumer implements. The driver pushes records into the sink. No DB dependency.

Alternatives considered: a binary CLI as the entrypoint. Rejected — we want the driver embeddable in the eventual MCP daemon. A separate thin CLI wrapper (`kenn index <workspace>`) is trivially added later.

### D2. SCIP parsing: `scip` crate (protobuf types) + `prost` streaming

Decode `.scip` files using the `scip` crate types over a `prost` reader that streams `Document` messages one at a time. We do **not** use the Sourcegraph `scip` Go CLI at runtime — only as a developer tool during the spike.

Alternatives considered:
- Spawn the Go `scip` CLI and parse its JSON output. Rejected — adds runtime dep, slower (extra serialize/deserialize), JSON output isn't streaming-friendly.
- Hand-roll protobuf decoding. Rejected — `scip` crate already provides the generated types.

### D3. Indexer dispatch: registry of `LanguageDriver` impls

A `LanguageDriver` trait with methods `discover(workspace) -> Vec<Unit>` and `run(unit) -> Result<scip_path>`. Concrete impls: `CSharpScipDotnet`, later `TypeScriptScipTs`, `PythonScipPython`, `GoScipGo`, `RustAnalyzerScip`.

Day-1 ships only `CSharpScipDotnet`. Trait definition is part of this proposal so it does not change when other languages land.

Alternatives considered: configure entirely via TOML (no Rust trait). Rejected — language-specific quirks (C# wants pre-restore, TypeScript wants `tsc --noEmit`, Rust wants `--all-features`) need code paths. A trait is the honest abstraction.

### D4. Identity model: `(canonical_path, symbol, range)` composite key

This is the only identity rule that survives the namespace-collision case (two "Common" projects). Symbol-string-only dedup would silently merge unrelated definitions; this rule keeps them apart.

Trade-off: a renamed file produces "different" symbols even though semantically identical. Acceptable for this proposal — the ingest layer can do post-hoc rename detection later if needed.

Alternatives considered:
- Synthesize a project-disambiguator and inject into the symbol string. Rejected — loses natural cross-`.sln` matching for legitimately shared projects, and the disambiguator (project name? path hash?) is itself a hard problem.
- Use SCIP's `symbol_string` only, ignore collisions. Rejected — silently merges unrelated symbols on real code.

### D5. Path canonicalization: workspace-relative storage

All paths in the data model are workspace-relative (`Worker/Host/Host.cs`, never an absolute path on the developer's machine). This makes a workspace's data portable and survives moves to different machines.

Conversion: `metadata.project_root` (URI) → absolute filesystem path → strip workspace root prefix. Records outside the workspace root are dropped with a warning.

Alternatives considered: keep absolute paths. Rejected — non-portable, leaks user paths into the data model.

### D6. Edge derivation: post-process per document

The "has-a/uses-a" edge derivation runs as a per-document pass over occurrences. For each non-definition occurrence, we need an `enclosing_range` (body of the containing AST scope) to find the FROM symbol via `occurrence ∩ defs whose range.start ∈ enclosing_range`.

**Enclosing-range provider (three-tier fallback, activated per-indexer):**

1. **SCIP `Occurrence.enclosing_range`** when the indexer populates it. Free, accurate.
2. **Language-specific positional refinement** (no AST parser) when SCIP leaves it empty. Reads the source file once and applies a small set of corrections to the bare last-preceding-def heuristic. For C# the corrections are:
   - Skip parameter-kind defs (descriptor leaf `(name)`) as FROM candidates — references inside a method body belong to the method, not its parameters.
   - Re-anchor occurrences whose source line is part of an attribute list (`[Attr]` / multi-line `[Attr(\n  ...\n)]`) to the next code line. Distinguish C# 12 collection literals (`x = [a, b, c]`) from attribute lists by checking whether the previous non-blank, non-comment line ends in an expression-continuation token (`=`, `=>`, `,`, `(`).
   - When the occurrence sits on the same line as a forthcoming def's identifier (e.g., `public BigDecimal AveragePrice` — `BigDecimal` precedes `AveragePrice`'s identifier column), pick the same-line forward def directly.
3. **Last-preceding-def heuristic** only when 1-2 leave no FROM (e.g., source file missing on disk; classifier produced no candidate).

Per-indexer activation (from the field-coverage probe in Context):
- **scip-dotnet, rust-analyzer**: `enclosing_range` empty on all defs → positional refinement (tier 2) required.
- **scip-typescript, scip-python, scip-go**: `enclosing_range` natively covers 100% of refs inside containers (verified on a TypeScript monorepo, click Python 92% inside-container hit, httprouter Go 96%). The 4-8% miss is exclusively file-scope refs → tier 3 fallback only.

The container-coverage finding is the empirical answer to "is partial enclosing_range enough?" — yes, because every ref that is "from a function/method" is covered, and the misses are by-definition not "from a function".

Each `LanguageDriver` (D3) declares whether it needs positional refinement (tier 2). C# and Rust will. TypeScript/Python/Go won't.

Empirical justification (a 303k-LoC C# repo C# spike, see `scratch/surreal-spike/results.md`):
- scip-dotnet emits `enclosing_range` empty on **all** 839k occurrences. (1) is unavailable in practice today.
- Bare last-preceding-def heuristic disagrees with tree-sitter on **20.5 %** of FROM attributions (after fixing the parameter-kind-defs bug — pre-fix: 26 %).
- The C# positional refinement (parameter-skip + attribute re-anchor + same-line forward-def + collection-literal disambiguation) brings agreement to **99.79 %**. Adds ~0.5 s to a 0.6 s pure-heuristic load (~1.8×). For comparison, tree-sitter post-processing was ~10× pure heuristic.
- Residual disagreements split as: enum-member attributes (the spike's tree-sitter doesn't recognize `enum_member_declaration`, so C is *more* correct than B here); cross-method-body refs in expression-bodied returns / inline lambdas; rare cases where tree-sitter itself was buggy. None of these is the original attribute case.

**No AST parser in the indexer.** Tier 2 reads the source file as text and runs a small line classifier. No grammar, no parse tree, no scope tracker. This permanently retires the question of bringing in a Tier-1 tree-sitter indexer.

Alternatives considered:
- **Last-preceding-def heuristic only.** Rejected — 20 % attribution error rate (post param-fix), wrong calls accumulate at common targets.
- **Tree-sitter post-processor.** Worked (~99 % match per the spike) but ~10× heuristic in cost, vendored grammar per language, and the spike's own implementation had two bugs (last-def-in-range vs. closest-to-enclosing-start; missing `enum_member_declaration` in decl kinds). The positional refinement reaches the same accuracy without the grammar dependency.
- **Upstream PR to scip-dotnet to populate enclosing_range.** Still desirable as a long-term fix — it would let us skip tier 2 for C#. Filed as future work (see Risks).
- **Roll our own scope tracker.** Rejected — overkill for the FROM-attribution problem; the line classifier is enough.

Edge `kind` is determined by an indexer-specific classifier:
- For SCIP: a heuristic on the SCIP-symbol descriptor terminator (`#` → type_use, `().` → calls, `.` → field_access, etc., when `SymbolInformation.kind` is unset — verified empty for scip-dotnet) plus position rules (`new T()` → `instantiates`). Default: `references`.

### D7. Multi-run merge: per-run materialization with provenance

Each indexer run produces a "run partition" identified by `(indexer, unit, timestamp)`. Records carry the `run_id`. Merge is done at query time (or at materialization time) by deduping on `(canonical_path, symbol, range, role)` across run partitions.

This means re-indexing one `.sln` updates only its partition. Other `.sln` partitions are untouched. Atomicity is per-partition.

Alternatives considered: single global table, full reindex on any change. Rejected — wasteful at scale and prevents incremental per-`.sln` reindexing.

### D8. Streaming Sink trait

```
trait Sink {
    fn begin_run(&mut self, report: &RunReport);
    fn write_symbol(&mut self, sym: SymbolRecord);
    fn write_occurrence(&mut self, occ: OccurrenceRecord);
    fn write_edge(&mut self, edge: EdgeRecord);
    fn end_run(&mut self, status: RunStatus);
}
```

Synchronous, back-pressure via the trait (consumer can block). Async is a wrapper concern; the producer doesn't need it.

### D9. Failure tolerance: per-project diagnostics, never abort

scip-dotnet emits per-project failures in stderr. The driver captures these by parsing stderr for `Microsoft.CodeAnalysis.MSBuild.MSBuildWorkspace[0] Failure:` lines and attaches them to the run report. A failed project just has fewer documents in the output; the rest still produces records.

If `scip-dotnet` itself crashes (no output file), the run is marked `failed`, no records emitted, error captured.

### D10. C# `Directory.Build.props` provisioning: opt-in

Driver detects MSBuildWorkspace failures with the vulnerability-error signature and surfaces a "would you like to add `<NuGetAudit>false</NuGetAudit>`?" hint in the report. Provisioning happens only on explicit user opt-in (CLI flag or config); we never silently rewrite the user's workspace files.

### D11. Worktree exclusion: git-aware, not glob-based

Worktrees are discovered from git, not from a path pattern. Users put worktrees in many places — `.worktrees/foo`, `../feature-x`, `~/wt/feature-x`, even paths chosen by tools — so a hard-coded `.worktrees/` glob would miss most of them and mis-exclude unrelated dirs that happen to share the name.

The driver runs the equivalent of `git worktree list --porcelain` from the workspace root. Each reported worktree path that is a strict descendant of the workspace root (and not equal to the workspace root itself) is added to the runtime exclude set. If the workspace root is itself a linked worktree, it is still indexed; only *other* worktrees descended from it are excluded.

When the workspace root is not in a git repository, this step is skipped and only the explicit configured exclude globs apply (`node_modules/`, `bin/`, `obj/`, `target/`).

Implementation: shell out to `git` via the `gix` crate (preferred — pure Rust, embeddable) or `std::process::Command::new("git")` if `gix`'s worktree-listing surface is incomplete. Either way the producer caches the worktree list per run, not per file.

Alternatives considered:
- Fixed glob `.worktrees/`. Rejected — too fragile; many users keep worktrees elsewhere.
- Walk filesystem looking for `.git` files (the marker that a directory is a linked worktree). Rejected — slower than asking git, and misses bare/odd configurations.

### D12. Concurrency: one indexer process at a time per language, units in parallel

scip-dotnet spawns a Roslyn workspace internally — running two scip-dotnet instances simultaneously could thrash the .NET runtime / NuGet cache. Default: serial within a language driver, parallel across language drivers. Configurable.

### D13. Symbol-id collision detection happens at the merge layer, not the parser

Per-document parse stays simple. The merge layer (where we materialize across runs) computes `(symbol_string) → set<canonical_path>` and emits a `symbol_collision` diagnostic when |set| > 1.

### D14. Symbol-kind classifier: SCIP descriptor grammar fallback

`SymbolInformation.kind` is unset in scip-dotnet, scip-typescript, and scip-python (verified per the Context coverage matrix). It is set in scip-go and rust-analyzer. Rather than write a per-language AST classifier, we derive kind from the SCIP symbol string itself when `kind` is unset.

The SCIP symbol grammar terminates each descriptor with a sigil that uniquely encodes its kind, the same way across all SCIP indexers:

| Suffix | Kind |
|---|---|
| `/` (trailing) | Namespace / Module / Package |
| `#` | Type (Class / Interface / Struct / Trait / Enum / Record) |
| `().` | Method / Function / Constructor |
| `.` (after a `#`-terminated parent or top-level) | Field / Property / Value / Constant |
| `(name)` | Parameter |
| `[T]` | TypeParameter |
| `+` | Macro |

The classifier parses the symbol's last descriptor and returns the kind. It is language-agnostic by spec and trivially testable.

Coarseness — the suffix grammar collapses Class vs. Interface vs. Struct vs. Record into "Type". For consumers that need finer distinctions, `Documentation` (markdown prose like `class AppError`) is a string-parsable secondary signal, but we do not parse it: distinguishing class from interface from struct is not on the day-1 query path. If a consumer later needs it, the indexer can parse `Documentation` for that one bit; today it's deferred.

Alternatives considered:
- **Tree-sitter for symbol kind.** Rejected — adds a per-language grammar dependency for what the SCIP symbol itself already encodes.
- **Wait for indexers to populate `kind`.** Rejected for day-1 — three of the five surveyed indexers don't, and we can't block on upstream.

## Risks / Trade-offs

- **[Risk] scip-* indexers go unmaintained.** Sourcegraph's pivot has slowed several. → Mitigation: pin versions; keep build-from-source fallback (we already proved this works). When an indexer's gaps are fillable with positional refinement (e.g., `enclosing_range`) or a descriptor-grammar classifier (e.g., `SymbolInformation.kind`), do it. We do NOT plan a full Tier-1 AST-based indexer as a fallback.

- **[Future work] Upstream PRs to populate gaps (Context table).** `Occurrence.enclosing_range` and `SymbolInformation.kind` exist in the SCIP schema; the indexer's host (Roslyn for scip-dotnet, rust-analyzer's HIR for Rust) has the data trivially. Once upstream populates them, the positional refinement and descriptor classifier become no-ops for those languages. Tracking; not blocking.
- **[Resolved] rust-analyzer relationships encoding.** Initial probe (BurntSushi/byteorder, 853 SymbolInfos, 0 relationships) raised concern about a missing emission path. Re-probe on quinn (quinn-rs/quinn, 10k SymbolInfos, 60k occurrences across 83 docs) confirmed: rust-analyzer encodes trait impls **as first-class impl-block symbols** following the pattern `mod/impl#[TYPE][TRAIT]` (with method members like `mod/impl#[TYPE][TRAIT]decode().`), not as `SymbolInformation.Relationships`. This is richer than the relationship form but requires a Rust-specific adapter to derive canonical `implements` / `override` edges. Tracked as a Rust-language-driver task; not blocking C# day-1.
- **[Risk] scip-dotnet bundles a .NET 8 SDK internally; some workspace projects target newer/older TFMs and fail with `MissingMethodException`.** Observed on the spike. → Mitigation: per-project failure tolerance handles it; affected projects are reported but other projects still index. Document this clearly. Consider building scip-dotnet against newer SDKs as an upstream contribution.
- **[Risk] Edge-kind derivation accuracy.** The heuristic classifier (D6) won't perfectly recover field-vs-param-vs-return-type from SCIP occurrences alone. → Mitigation: start with a small high-confidence ruleset + `references` fallback. The agent-consumer tolerates approximate edge kinds; precision can grow as we measure where it matters.
- **[Risk] Path canonicalization edge cases.** Symlinks, case-insensitive filesystems on macOS, junctions on Windows. → Mitigation: use `std::fs::canonicalize` consistently; document case-folding behavior; add a regression test per OS.
- **[Risk] Memory blow-up on huge `.scip` files.** Tier-2 output for a 1M LoC monorepo could hit 220+ MB. → Mitigation: streaming protobuf parse (D2), one document at a time. Add a per-document memory budget assertion in tests.
- **[Risk] Workspace-relative paths assume no symlinks crossing the root.** A symlinked subdir pointing outside breaks the prefix-strip. → Mitigation: canonicalize before stripping; warn on out-of-tree paths.
- **[Trade-off] Day-1 has no incremental indexing.** Every change re-runs the full `.sln`. → Acceptable: scip-dotnet on cached restore is fast (~91s for 303k LoC). Incremental is a follow-up.
- **[Trade-off] No symbol-rename detection.** A renamed file produces "new" symbols. → Acceptable: rare event; detection can be added at the ingest/query layer.

## Migration Plan

This is a greenfield codebase. No migration. Crate ships fresh.

## Open Questions

- **Configuration format**: TOML at workspace root (`kenn.toml`)? Or env vars? Or CLI-only? — Defer to first implementation; defaults are usable without config for the C# happy path.
- ~~**Where does the persisted run partition live before the DB exists?**~~ Resolved during implementation: storage is SurrealDB-only; the producer streams records directly into a `SurrealdbSink` (impl lives in `indexed-store-and-lifecycle`). No JSONL intermediate. Atomicity is SurrealDB's responsibility (transactional ingest).
- **scip-dotnet packaging**: pinned Docker image, dotnet tool install, or build-from-source? — All three worked in the spike. Suggest dotnet tool install (`dotnet tool install --global scip-dotnet`) as default since it's fastest on the Mac dev path. Document the build-from-source escape hatch.
- **How do we handle .csproj-only workspaces (no .sln)?** scip-dotnet's `index` subcommand accepts project paths. → Add later when we hit it; not blocking day-1 monorepo case.
