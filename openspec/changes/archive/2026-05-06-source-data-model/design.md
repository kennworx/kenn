## Context

The system has been driven by three prior threads:

1. The `scip-indexing-pipeline` proposal defines record production from SCIP indexers. It produces normalized records but leaves the public contract unspecified.
2. The `indexed-store-and-lifecycle` proposal defines storage atomicity, ingest pipeline, and lifecycle, but leaves the logical schema as an open question.
3. Two query/use-case investigations — agent transcripts (`scratch/mcp-design/transcripts.md`) and a query-shape probe against real C# spike data (`scratch/mcp-design/query-probe.md`) — articulated what consumers actually need.

Empirical anchors that shape this design:

- **Graph relations are ~1000× faster than table-encoded edges for multi-hop queries** (query-probe; depth-7 traversal: 0.21–0.7 ms graph vs. 800+ ms table).
- **BM25 fuzzy search is ~20× faster than `string::starts_with` prefix search in SurrealDB** (query-probe; 2 ms vs 45 ms). Prefix-based lookup of any kind is avoided where BM25 or exact-match suffices.
- **External-symbol references (e.g., `System.String.Format`) can blow up result sets** (query-probe). Filter by `is_external` on hot paths.
- **scip-dotnet emits `enclosing_range` empty on every occurrence**, scip-typescript/python/go emit it on container defs (~12-26% coverage matches container fraction), rust-analyzer emits it empty (`scip-indexing-pipeline/design.md`). Coverage is "complete enough" for the goal of attributing FROM=container, the rest is file-scope refs.
- **rust-analyzer encodes trait-impl relationships structurally as `impl#[Type][Trait]` symbols, not as `SymbolInformation.Relationships`** (scip-indexing-pipeline/design.md). Per-language adapters derive canonical edges from indexer-specific encodings.
- **SCIP `SymbolInformation.kind` is unset in scip-dotnet, scip-typescript, scip-python**; populated in scip-go and rust-analyzer. Symbol kind is derivable from the SCIP descriptor grammar as a universal fallback.

This design covers only the **logical model** — what the data IS. Storage layout, atomicity, and the SCIP-to-record transformation live in their respective proposals.

## Goals / Non-Goals

**Goals:**

- A public symbol ID that is stable across file rename and code move within the same module (where languages permit), debug-readable, and language-native in syntax.
- An internal schema that supports the agent query patterns surfaced in `transcripts.md` at the latencies measured in `query-probe.md`.
- Multi-language support from day one — schema does not change as new languages are added.
- A wire location format simple enough that agents pass it back as a string handle.
- Explicit deferral of features that aren't worth v1 cost.

**Non-Goals:**

- DB technology selection (covered in `indexed-store-and-lifecycle/specs/index-store-db`).
- Storage layout (`indexed-store-and-lifecycle/specs/index-store-layout`).
- API surface — MCP tools, response shapes, pagination, ranking (future `mcp-server` proposal).
- SCIP transformation rules (`scip-indexing-pipeline`).
- Source-text retrieval and snippet caching — agents use their own file-read tools.
- LSP / live-query bridge.
- Auto-detected isomorphism (`corresponds_to` is config-only in v1).
- Data-flow analysis. Cross-language type isomorphism inference.
- Exception-handling edges (`throws`, `handled_by`). SCIP doesn't carry exception data; tree-sitter approach deferred.
- Site-level call/use queries — edges are pair-deduped; agents read source for site detail.

## Decisions

### D1. Public ID format: language prefix + native syntax

Public symbol IDs follow the form `<lang>:<native-id>`:

```
cs:Models.Order.Foo(string)
ts:@some-org/frontend-shared/api.AppError
rs:quinn_proto::connection::Connection::new
go:quinn-proto/connection.Connection.New
py:click.core.Context.invoke
```

The lang prefix is two-letter compact (matches the file-suffix conventions). The remainder is what a developer in that language community would write to refer to the symbol — namespace/package path, optional generic arity, optional overload signature (C# only).

Rules:
- **C#**: `Namespace.Type.Member(paramTypes)`. Project name is metadata, not in ID.
- **Rust**: `crate::path::to::item`. No turbofish in canonical IDs (`Foo<T>`, not `Foo::<T>`).
- **Go**: `package_path.Symbol` or `package_path.Type.Method`. Package path is the import path.
- **TypeScript**: `<package>/<file-without-ext>.Symbol`. Module is bound to file by language semantics; rename = ID change (unavoidable).
- **Python**: `module.Class.method`. Distribution is metadata.

**Rationale.** Native syntax means agents see what they'd write. Lang prefix is small overhead and disambiguates collisions across languages. Round-trips to/from SCIP cleanly via per-language transformers. Verbatim SCIP strings are NOT exposed publicly — they leak indexer choice (`scip-typescript`, `nuget`, etc.) and version.

**Stability rules:**
- File rename → ID stable (file path is not in ID, except TS/JS where modules ARE files).
- Symbol moved within same file → ID stable.
- Symbol moved across files (same module, where languages support it) → ID stable.
- Symbol renamed → ID changes (semantically correct: the symbol no longer exists at the old name).
- Symbol moved across modules → ID changes (semantically correct).

When an agent holds a stale ID and queries a rebuilt index, the response is `not_found`. Suggestion engines (parent + kind heuristics) are a v2 enhancement; v1 returns 404 cleanly.

Alternatives considered:
- Verbatim SCIP symbol strings as public IDs. Rejected — leaks indexer prefix and package version into the public contract; ugly.
- Opaque hash IDs. Rejected — debug-hostile; can't survive index rebuild without extra mapping work.
- Uniform format across languages (e.g., `csharp/Order/Foo`). Rejected — feels alien to every language community; agents see code in native syntax; IDs should match.

### D2. Internal short_id u32 for all cross-references

Inside the DB, every cross-reference uses `u32` short ids: `files.id`, `symbols.short_id`, foreign-key columns (`enclosing_symbol`, `file`), relation source/target endpoints, and side-table parents.

The public string ID lives only as `symbols.id` (with `(language, id)` indexed UNIQUE for API-boundary lookup). Translation `short_id ↔ public id` happens only at the API boundary.

**Rationale.** At 1M-LoC scale, the difference between u32 (4 bytes) and string (~50 bytes for a public ID) on every cross-reference is meaningful: ~12× storage savings on FK-heavy tables (occurrences-during-ingest, relations).

Auto-increment IDs start at 1; **0 is a reserved sentinel for "no reference"** (e.g., top-level symbols whose `enclosing_symbol = 0`). This avoids nullable columns entirely.

### D3. No nullable columns

All columns have a default value. Sentinel values express "absent":

| Column | Type | Sentinel for absent |
|---|---|---|
| `enclosing_symbol` | `u32` | `0` |
| `file` | `u32` | `0` (synthetic / external symbol) |
| `args_arity` | `u8` | `0` (kind disambiguates "not callable" vs "0 args") |
| `generic_arity` | `u8` | `0` (kind disambiguates "not generic" vs "0 generics") |

Booleans default to `false`. Strings default to empty.

**Rationale.** Single-byte storage for u8 columns (no nullable bitmap overhead). Simpler query semantics — never need IS NULL checks. Filters use the kind column to disambiguate when `0` is ambiguous: "callable methods" → `WHERE kind IN ('method', 'function', 'constructor')`, not `WHERE args_arity > 0` (which would exclude nullary).

### D4. files table: separate, with content_hash

Files are first-class. Symbols and relations reference `files.id` (u32) — never path strings.

```
TABLE files {
  id            u32 PK auto-increment
  path          string                -- workspace-relative, canonical
  language      string
  is_test       bool
  is_external   bool
  content_hash  u64                   -- xxhash64 of file contents
}
```

**Rationale for the table itself:**
- Storage: at 1M LoC, ~10k files × ~50 B path string = 500 KB once, vs. ~225 MB if denormed onto every symbol/occurrence. ~450× savings on path-bearing rows.
- Per-file metadata (`is_test`, `is_external`, `language`) lives once per file. Reclassification (e.g., changing the test glob) updates one row, not millions.
- `module ↔ file` relation needs file as a queryable entity, not a free-form path string.
- File metadata changes on a different cadence than symbol metadata — separating cleanly cleanly limits update churn.

**Rationale for `content_hash` (xxhash64):**
- Skip re-indexing files whose content matches the stored hash (incremental ingest).
- Detect identical content across worktrees (cross-worktree dedup at scale).
- Validate worktree-fallback safety (`indexed-store-and-lifecycle/D7-D8`).
- xxhash64: ~10 GB/s, 8 bytes; consistent with the indexed-store proposal's git-aware skip choice.

### D5. symbols table: wide, with denormalized hot-path fields

```
TABLE symbols {
  short_id          u32 PK auto-increment      -- internal handle
  id                string                      -- PUBLIC normalized native ID (D1)
  language          string                      -- "csharp" | "typescript" | "rust" | "go" | "python"
  kind              enum                        -- D7
  name              string                      -- short form (last meaningful descriptor)
  display_name      string                      -- pretty form for agent display
  enclosing_symbol  u32 default 0               -- direct parent (any kind); 0 = top-level
  file              u32                          -- primary def file (denorm)
  def_range         [u32; 4]                    -- primary def range (line, col, line, col)
  is_partial        bool                         -- true if additional defs exist in partial_defs
  args_arity        u8 default 0                 -- count of formal params (callable kinds only)
  generic_arity     u8 default 0                 -- count of generic params
  is_external       bool                         -- defined outside the workspace
  is_test           bool                         -- denorm of files.is_test
}
```

**Rationale.** This table is the hot-path. Every API response needs id, language, kind, name, display_name, plus location (file + def_range). Denormalizing them avoids JOINs on the hot path. Sparse fields (signature_doc, documentation) move to `symbol_docs`.

`enclosing_symbol` is a column (1:1 parent link, ~4 bytes per symbol vs. ~50 bytes if a graph relation row). Direct-parent lookup is O(1) FK. **Subtree traversal goes through the `defined_in` relation** (D8) — different concern, different mechanism.

`primary def` fields are denormed for the 99% case where a symbol has exactly one definition. Partial classes (rare; mostly C#) use the `is_partial` flag + the `partial_defs` side table (D6).

### D6. symbol_docs (sparse) and partial_defs (rare) as side tables

```
TABLE symbol_docs {
  symbol           u32 PK              -- → symbols.short_id
  signature_doc    string              -- e.g. "fn foo<T>(x: T) -> T" (markdown-ish)
  documentation    string              -- doc comments / docstrings
}
INDEX documentation USING bm25         -- intent search ("find symbols that handle auth")

TABLE partial_defs {
  symbol           u32                  -- → symbols.short_id
  file             u32                  -- → files.id
  range            [u32; 4]
  PRIMARY KEY (symbol, file, range[0])
}
INDEX symbol
INDEX (file, range[0])
```

`symbol_docs` is split because:
- Density varies wildly: scip-go and rust-analyzer populate `signature_doc` 100%; scip-dotnet, scip-typescript, scip-python populate it 0% (verified per `scip-indexing-pipeline/design.md`).
- Sizes are larger (200 B – 2 KB for documentation strings) and the hot-path "find by name" / "lookup by ID" doesn't need them.
- BM25 over `documentation` is a separate query path (intent search) from BM25 over `symbols.name` (name search).

`partial_defs` is split because:
- For 99% of symbols, exactly one definition exists. `symbols.file` + `symbols.def_range` cover them.
- C# partial classes (and rare equivalents) need additional def locations. `is_partial = true` flags the side-table lookup.
- For non-partial symbols, the row count in `partial_defs` is zero — no overhead.

### D7. kind enum (closed set)

```
package, module, namespace,
class, struct, interface, trait, enum, enum_member, type_alias,
method, function, constructor, destructor, operator,
field, property, constant, variable,
parameter, type_parameter,
macro
```

Distinct values for `module` vs `namespace` (Rust/Go vs C#). A `package` is the top-level grouping (cargo crate, npm package, .csproj, Go module, Python distribution). `enum_member` is a value of an enum type (a Rust enum variant or a C# enum constant); enums themselves are kind=`enum`.

**Rationale.** A closed set keeps queries deterministic ("find all methods" doesn't depend on what the indexer happened to call them). The enum is broad enough to absorb future languages without schema change.

Mapping from indexer:
- When SCIP `SymbolInformation.kind` is set (scip-go, rust-analyzer): map directly via a per-indexer table.
- When unset (scip-dotnet, scip-typescript, scip-python): derive from the SCIP symbol descriptor suffix (`#` → type, `().` → method/function, `.` → field/property, `(name)` → parameter, `[T]` → type_parameter, trailing `/` → namespace/module/package). Per `scip-indexing-pipeline/D14`.

### D8. defined_in relation: symbol → module/package (semantic location)

```
RELATION defined_in       FROM symbols TO symbols
                          -- target: kind ∈ {package, module, namespace}
                          -- source: any non-(package/module/namespace) kind
                          --         OR a (sub)module/namespace nested inside another
```

Every symbol that lives in a module/namespace/package gets one `defined_in` edge to its **most specific** module/namespace ancestor. For nested modules: child module's defined_in points to its parent module. For top-level modules in a package: defined_in points to the package.

Queries:

```
"what module contains symbol X"          SELECT ->defined_in->* FROM symbols:X
"all direct children of module M"        SELECT <-defined_in<-* FROM symbols:M
"all symbols in module subtree"          SELECT <-defined_in<-..* FROM symbols:M  (recursive)
```

**Rationale.** A relation, not a denorm column, because:
- Bidirectional traversal: "module of X" and "all symbols in M" are equally cheap (both 1-hop or recursive-fast).
- Recursive subtree queries via `<-defined_in<-..*` perform at single-digit ms even for deep hierarchies (validated against the spike's depth-7 measurement: 0.21 ms; query-probe; schema-probe). A column denorm couldn't power "all in subtree" without extra denorm columns or recursive WHERE (slow).
- Storage cost is a relation row per non-package symbol: ~25 B × 500k = ~12 MB at 1M-LoC scale. Comparable to a denorm column.

Top-level packages have no `defined_in` edge (they have no enclosing scope).

### D9. contains relation: module ↔ file (physical layout, M:N)

```
RELATION contains         FROM symbols TO files
                          -- source: kind ∈ {module, namespace, package}
                          -- many-to-many: a module can span multiple files; a file
                          --   can contain multiple modules (rare, but valid)
```

This is the **physical** counterpart to D8's semantic `defined_in`. A module's source is laid out across one or more files; a file's content belongs to one or more modules.

Queries:

```
"files implementing module M"            SELECT ->contains->* FROM symbols:M
"modules in file F"                      SELECT <-contains<-* FROM files:F
```

**Rationale.** The semantic-vs-physical split lets module hierarchy and file layout evolve independently. `defined_in` does not change when a Rust mod splits across two files; `contains` does. Conversely, renaming a file (without moving its symbols out of the module) updates `contains` but not `defined_in`.

Per-language realities:
- TypeScript: module = file. The relation is ~1:1.
- Python: module ≈ file, with `__init__.py` aggregating a package's submodules. ~1:N for packages, ~1:1 for leaf modules.
- Go: package = directory; one package spans every `.go` file in that directory. 1:N.
- Rust: a `mod foo;` in `lib.rs` resolves to either `foo.rs` or `foo/mod.rs` plus optional siblings. 1:N.
- C#: a namespace can be declared in any file (and a single file can declare multiple namespaces). M:N.

### D10. Symbol-level relations (deduplicated at pair granularity)

```
RELATION calls               FROM symbols TO symbols
RELATION type_use            FROM symbols TO symbols
RELATION field_access        FROM symbols TO symbols (op: read | write)
RELATION implements          FROM symbols TO symbols
RELATION overrides           FROM symbols TO symbols
RELATION instantiates        FROM symbols TO symbols
RELATION generic_constraint  FROM symbols TO symbols    -- T → ConstraintType
                                                        -- (e.g. fn foo<T: Bar>: T → Bar)
```

**Pair deduplication.** For each `(source, target, kind)`, exactly one relation row exists. If MethodA calls MethodB at three different lines, there is **one** `calls` edge.

**Rationale.**
- Edge volume drops by ~3× compared to per-call-site storage (spike: 600k edges → ~200k after dedup).
- Agent's natural unit of reasoning is "MethodA depends on MethodB" (semantic), not "MethodA calls MethodB at lines 42, 56, 91" (syntactic). The semantic version is what shapes refactor decisions.
- Site-level recovery, when needed, is reading the caller's source — the agent's existing file-read tool handles that. Our schema does not store call sites.

What's lost: count of call sites and exact site positions. What's preserved: every relationship at semantic granularity. The transcript exercise shows agents reason in terms of "8 distinct callers" rather than "47 call sites" — the deduped form matches the agent's mental model.

### D11. corresponds_to relation: cross-boundary symbol equivalence

```
RELATION corresponds_to   FROM symbols TO symbols
                          (source:    enum,         -- "config" | "auto-inferred" | "codegen"
                           generator: string,        -- e.g. "protoc", "openapi", "manual" — empty if not codegen
                           canonical: u32 default 0  -- short_id of the source-of-truth symbol, if any
                          )
```

Used for "these symbols represent the same logical entity across a boundary":
- Cross-language (a Rust struct manually duplicated in TypeScript).
- Codegen targets (`.proto` struct → generated `cs:`/`ts:`/`py:`/`go:`/`rs:` types).
- Versioned migrations (a v1 type and a v2 type known to be the same logical entity).

`canonical` (when set) points to the source-of-truth symbol — e.g., the protobuf def. This makes the relation morally directional even though the rows are bidirectional in graph form. `0` indicates no canonical source.

v1 ships **config-declared only** (`source = "config"`). Codegen-detection and auto-inference are deferred to a follow-up proposal.

### D12. imports relation: module → module (semantic; M:N)

```
RELATION imports          FROM symbols TO symbols
                          (kind: enum)         -- "explicit" | "re_export"
                          -- both endpoints: kind ∈ {package, module, namespace}
```

One row per (importing module, imported module) pair, regardless of how many import statements exist.

**Rationale.**
- Module-level granularity is what package-extraction queries (transcript T4) actually need. "Does package payment import package order?" is the question; line-level imports are an implementation detail of the file.
- Where the import statement physically lives is recoverable by reading the importing module's files (via `contains`) — the agent's file-read tool handles that.
- Re-exports (e.g., a Rust `pub use` or a TS `export * from`) are flagged via `kind = "re_export"` so consumers can distinguish "uses internally" from "re-exposes."

### D13. Wire location format: `./file_path#start-end`

Every API response that includes a location uses this format:

```
./src/api.ts#42                  -- single line
./src/api.ts#3-14                -- range (def of a class spans lines 3-14)
null                              -- synthetic / no def location (external, etc.)
```

**Rationale.**
- Agent passes the string back as a handle; no separate parsing of file/line/range fields.
- Workspace-relative; `.` prefix makes it visually clear it's not an absolute path.
- Line-level only: column precision is rarely useful at the agent layer (the file content is what they actually inspect). Column data still lives in `def_range` for any consumer that needs it.

### D14. Occurrences are an ingest-time intermediate, not a persisted table

The producer pipeline materializes occurrences (every appearance of a symbol with role and enclosing-symbol attribution) **in memory** during ingest. From them, the relations in D10/D12 are derived (deduplicated). After derivation, occurrences are discarded; the live DB does not have an occurrences table.

```
        .scip files
              │  parse + classify
              ▼
       ┌────────────────────────┐
       │ in-memory occurrences  │   ← transient
       └────────┬───────────────┘
                │  derive (FROM-attribution per occurrence)
                ▼
       ┌────────────────────────┐
       │ deduplicated relations │   ← persisted
       └────────┬───────────────┘
                │  persist
                ▼
            live DB
```

**Rationale.**
- The occurrence table has high cardinality (~8× symbol count; spike: 89k symbols → 736k occurrences). At 1M-LoC scale: ~4M rows, ~100 MB.
- Every consumer query that's been articulated is answerable from the deduplicated relations + source-text reading. Cursor-on-use-site is the only IDE-like capability lost; we are explicitly not building an IDE.
- For development iteration on edge classification rules: re-derive from the kept `.scip` files (`indexed-store-and-lifecycle/D1`) via re-parse. Slower than re-querying a kept occurrences table but acceptable for the rare schema-change case.

A debug mode persists occurrences to a rocksdb staging DB (toggled by `kenn index --debug-staging`); default is in-memory.

### D15. Test-file marking via glob config

```
# kenn.toml
[tests]
paths = [
  "tests/**",
  "**/*Test.cs",
  "**/*_test.go",
  "**/test_*.py",
  "**/*.test.ts",
  "**/*.spec.ts",
]
```

At ingest, files matching any glob are flagged `is_test = true`. The flag is denormalized onto symbols (`symbols.is_test = files.is_test`) for fast filtering.

**Rationale.**
- Per-language auto-detection (xunit attrs, `#[test]`, pytest, `_test.go` suffix) is per-language work that user globs handle uniformly today.
- Most agent queries want to filter tests OUT (`AND is_test = false`); a single denormed flag makes this a 1-byte column check, no JOIN.
- Reclassification (changing the glob list) updates `files.is_test` then re-denorms onto symbols — bounded work.

### D16. Indexes (Tier-A: minimal, query-driven)

```
files
  INDEX path UNIQUE
  INDEX (language, is_test)
  INDEX content_hash

symbols
  INDEX (language, id) UNIQUE                -- public-id lookup at API boundary
  INDEX (language, name) USING bm25          -- name search (~2 ms verified)
  INDEX (language, kind)                     -- "find C# methods named X"
  INDEX file                                 -- "what's defined in this file"
  INDEX enclosing_symbol                     -- "all members of this parent"

symbol_docs
  INDEX documentation USING bm25             -- intent search

partial_defs
  INDEX symbol
  INDEX (file, range[0])

defined_in        (relation; SurrealDB indexes in/out automatically)
contains          (relation; auto)
calls / type_use / field_access / implements / overrides / instantiates / generic_constraint
                  (relations; auto)
corresponds_to    (relation)
                  -- INDEX source on the relation table  (deferred unless filtered queries are hot)
imports           (relation)
                  -- INDEX kind on the relation table     (deferred unless filtered queries are hot)
```

**Rationale.** Each index is justified by an articulated query in `transcripts.md` or `query-probe.md`. Property indexes on relations are deferred per the schema-probe finding: traversal-time filters are already fast (<1 ms); table-scan property filters benefit only marginally from indexes.

### D17. Multi-language in one schema

The schema is multi-language by default. Symbols, relations, and indexes carry a `language` column where relevant; per-language tables are NOT created.

**Rationale.** Cross-language queries (transcripts T3) become trivial. Per-language splits would force UNIONs everywhere and prevent cross-language relations (`corresponds_to`, multi-lang call chains) from being expressible at all in the schema.

Per-language ID transformers live in the producer side; they translate SCIP symbols to public IDs at ingest. The schema itself is uniform.

## Risks / Trade-offs

- **[Risk] No site-level call positions.** Lost capability: "where exactly does MethodA call MethodB?" → reconstructed by reading MethodA's source. Mitigation: this is the agent's natural workflow anyway.
- **[Risk] No cursor-on-use-site lookup.** Lost capability: "what symbol is referenced at file:line:col?" beyond the def position. Mitigation: stack-trace-style debugging works via `def_range` (find the containing method); position-precise reverse lookup is genuinely lost. Acceptable per the "we are not an IDE" stance.
- **[Risk] `defined_in` recursive subtree on very wide modules** could return huge result sets. Mitigation: pagination is a query-layer concern (mcp-server proposal); the relation traverses fast regardless of result size.
- **[Risk] Public ID changes on rename.** Stale agent-held IDs return 404 after reindex. Mitigation: clean 404 in v1; suggestion engine (parent + kind heuristic) deferred. Agents retry with `find_symbol_by_name`.
- **[Risk] TS file rename = ID change.** Unavoidable property of TS's file-as-module semantics. Mitigation: documented limitation.
- **[Risk] rust-analyzer impl-symbol pattern requires a Rust-specific adapter.** Mitigation: producer-side concern (`scip-indexing-pipeline`); the schema accepts the resulting `implements`/`overrides` edges either way.
- **[Trade-off] Wide symbols table.** Some fields are sparse (e.g., `args_arity` is meaningless for kind=class). Acceptable: 1 byte per row × 500k rows = 500 KB extra. Splitting would JOIN every fetch.
- **[Trade-off] `corresponds_to` v1 is config-only.** Auto-inference and codegen-detection are deferred. The relation's shape is in place; a follow-up proposal can add inference without schema change.

## Migration Plan

Greenfield. No migration. Producer (`scip-indexing-pipeline`) and storage (`indexed-store-and-lifecycle`) reference this schema; both ship after this proposal lands.

## Open Questions

- **`corresponds_to` property indexing performance under real load.** Schema-probe used a proxy relation; actual indexed filtering on `source` should be re-measured once `corresponds_to` data exists.
- **Docs-table BM25 versus a separate FTS engine.** SurrealDB BM25 was fast on `name`; `documentation` will be longer text. Verify p99 holds at 1M-LoC scale before committing (revisit during the indexed-store bake-off integration).
- **Recursive `defined_in` traversal on extremely deep hierarchies.** Spike measured up to depth 7 cleanly. Pathological projects (deep namespace nests) might push depth 10+. Validate when first ingested.
- **`enclosing_symbol` for top-level package symbols.** Sentinel `0` works, but downstream queries (e.g., "find all top-level packages") must filter `WHERE kind='package' AND enclosing_symbol=0`. Documented; verify ergonomics in mcp-server proposal.

## Deferred Capabilities

Consolidated list of capabilities intentionally **not** in v1, with the
follow-up proposal that should pick each up:

- **Data-flow analysis** — value/type propagation across calls. Distinct from `type_use`/`calls`. Future proposal.
- **Exception-handling edges** — `throws`, `handled_by`. SCIP doesn't carry exception data; tree-sitter-based extraction is a follow-up.
- **Snippet blob cache** — pre-rendered source ranges for fast preview. Agents currently use their own file-read tools.
- **Codegen-detected `corresponds_to`** — v1 ships `source = "config"` only. Auto-inferring across `.proto`/`.openapi` boundaries is a follow-up.
- **Auto-inferred `corresponds_to`** — cross-language type isomorphism without explicit config. Same shape as codegen-detection but driven by structural matching.
- **Suggestion engine for not-found IDs** — `not_found.parent_id`/`parent_kind` hints land in v1; smarter "did you mean…" comes later (mcp-server v2).
- **Site-level call positions** — schema deduplicates at pair granularity. If agent workflows ever require call-site lookups directly from the DB, a follow-up adds an `occurrences_v2` table or restores the ingest-time occurrences as a queryable side-table.
- **Cursor-on-position lookup** — "what symbol is referenced at file:line:col?" is genuinely lost; we are not an IDE.
- **Property indexes on `corresponds_to.source` and `imports.kind`** — added when filtered queries become hot.
