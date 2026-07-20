## Purpose

This spec defines the JSONL wire format and streaming semantics for a
Roslyn-based C# indexer that produces symbol definitions, references, and
call graphs without intermediate SCIP serialization. It covers the producer
(the .NET indexer emitting frames), the consumer (Rust ingest code reading
frames and writing to SurrealDB), and concrete scenarios for both.
## Requirements
### Requirement: JSONL Wire Format

The dotnet-stream-indexer SHALL emit a sequence of newline-delimited JSON
frames on stdout. Each frame MUST be a single JSON object on a single line,
ending with `\n`, and MUST carry a `type` field that names the frame's
shape.

The frame types are: `meta`, `file`, `symbol`, `partial_def`, `edge`,
`error`, `end`. Producers SHALL emit exactly one `meta` frame as the
first frame, exactly one `end` frame as the last frame, and any number
of the others in between, in any order subject to the cross-reference
rules below.

Cross-references between frames SHALL use producer-assigned numeric `id`
values only. Both files and symbols share a single `u32` id space. The
reserved value `0` means "no reference". String identifiers (e.g.
SCIP-style `pub_id`) MUST NOT appear on the wire — they are an internal
producer dedup detail. Stable cross-run identity is the consumer's
concern, derived from observable fields like `path`, `name`, `kind`, and
the `parent` chain.

#### Scenario: First frame is meta, last frame is end

- **WHEN** the indexer runs against a non-empty workspace
- **THEN** the first line of stdout deserializes to a `meta` frame
- **AND** the last line of stdout deserializes to an `end` frame

### Requirement: Run-bracket frames carry timestamps

The `meta` (first) and `end` (last) frames SHALL each carry a `ts`
field — an ISO 8601 UTC timestamp recorded by the producer at the
moment the frame was written. Format: `YYYY-MM-DDTHH:mm:ss.sssZ`
(millisecond precision, `Z` suffix). Consumers MAY use the difference
between `MetaFrame.ts` and `EndFrame.ts` as the producer's wall-clock
duration without trusting per-frame producer-side bench instrumentation.

The field is required on both frames. Other frame types do not carry
`ts`; per-frame timestamps would bloat the wire to no benefit.

#### Scenario: Timestamps are present and parseable

- **WHEN** the indexer emits a `meta` frame
- **THEN** the frame contains a `ts` field that parses as ISO 8601
  with millisecond precision and `Z` (UTC) suffix
- **AND** the `end` frame at end-of-stream carries the same shape
- **AND** `EndFrame.ts >= MetaFrame.ts`

#### Scenario: Each frame is one line of JSON

- **WHEN** the indexer emits any frame
- **THEN** the frame is a single line of JSON terminated by `\n`

#### Scenario: Cross-references are numeric ids

- **WHEN** an `edge` frame references its endpoints
- **THEN** it uses `source` and `target` numeric fields
- **AND** no field on any frame contains a string `pub_id`/`source_pub_id`/`target_pub_id`

### Requirement: Producer Output Batching

The producer SHALL buffer frames and flush stdout in batches rather than
flushing after every line. Default batch threshold SHALL be a
configurable byte size (default: 1 MiB) or frame count (default: 4096),
whichever is hit first. The producer SHALL flush on `end` frame emission
and on process exit, including non-zero exit paths, so no buffered data
is lost.

#### Scenario: Producer flushes on end frame

- **WHEN** the indexer emits the `end` frame
- **THEN** the producer flushes stdout before the process exits

#### Scenario: Producer flushes when batch threshold is hit

- **WHEN** the buffered byte count exceeds the configured byte threshold
  OR the buffered frame count exceeds the configured frame threshold
- **THEN** the producer writes the buffered bytes to stdout and clears
  the buffer

### Requirement: Producer CLI Flags

The producer SHALL expose CLI flags to configure its behavior. Required
flags:
- A positional or `--workspace <dir>` argument selecting the workspace
  root (default: current working directory).
- `--projects <path>...` to override solution/project discovery
  (zero-or-more files).
- `--include <glob>...` and `--exclude <glob>...` for file-path filtering.
- `--skip-restore` to disable the implicit `dotnet restore` and
  `--restore-timeout-ms <int>` to bound it.
- `--flush-bytes <int>` and `--flush-frames <int>` to override the
  producer-batching thresholds.
- `--edge-kinds <list>` to enable a subset of edge kinds (default: all
  v1 + expansion kinds the producer supports).
- `--output <path>` to additionally tee the JSONL to a file for
  debugging (stdout is still emitted; absent: no file written).
- `--log-level <trace|debug|info|warn|error>` (default: `info`, written
  to stderr — stdout is reserved for JSONL).

#### Scenario: --workspace selects the indexed root

- **WHEN** the producer is invoked with `--workspace /path/to/repo`
- **THEN** project discovery starts from `/path/to/repo`

#### Scenario: --edge-kinds filters emitted edges

- **WHEN** the producer is invoked with `--edge-kinds defined_in,calls`
- **THEN** every emitted `edge` frame has `edge_kind` in
  `{defined_in, calls}`

#### Scenario: --output writes the same JSONL to a file

- **WHEN** the producer is invoked with `--output /tmp/idx.jsonl`
- **THEN** the file at `/tmp/idx.jsonl` contains exactly the bytes
  written to stdout

### Requirement: Producer-Assigned Numeric IDs

The producer SHALL assign every emitted file and symbol a numeric `id`
(`u32`) drawn from a single id space shared across files and symbols.
The producer guarantees that each conceptually-distinct file or symbol
gets exactly one id for the duration of the run. Method overloads SHALL
receive distinct ids — the producer's internal dedup key MUST disambiguate
overloads by parameter signature.

The producer's dedup mechanism (e.g. an internal pub_id-shaped key) is
implementation detail and never appears on the wire. Cross-run identity
(matching the same symbol across reindex runs) is the consumer's
responsibility, derived from observable fields on emitted frames.

The producer MAY emit the same id more than once: a "stub" frame
introduces the id with minimal information when the indexer needs to
reference the symbol before fully describing it; a later "full" frame
re-emits the same id with complete fields. The consumer SHALL UPSERT
keyed on `id`, so later frames overwrite earlier ones in place.

#### Scenario: Two overloads receive distinct ids

- **WHEN** a class declares both `void Bar()` and `void Bar(int x)`
- **THEN** the two methods are emitted as two `symbol` frames with two
  distinct `id` values

#### Scenario: Partial class shares one id across files

- **WHEN** a partial class is split across two source files and both are
  indexed
- **THEN** both source files contribute to a single `symbol` frame `id`
- **AND** at least one `partial_def` frame is emitted referencing that
  `id` to record the secondary declaration site

#### Scenario: Stub-then-upgrade reuses id

- **WHEN** the producer emits a stub `symbol` frame with `id: 42` while
  walking a method body, then later walks the symbol's declaration
- **THEN** the producer emits a second `symbol` frame with the same
  `id: 42` carrying the full declaration data
- **AND** the consumer UPSERTs both into the same row, last-frame-wins

### Requirement: def_range Is Populated for Every Symbol

Every `symbol` frame SHALL include a `def_range` field containing
`[start_line, start_col, end_line, end_col]` with 0-based line and column
indices, taken from `ISymbol.Locations[0].GetMappedLineSpan()` of the
declaring syntax. Synthetic symbols (the root package per assembly) MAY
omit `def_range`; in that case the consumer SHALL store `[0,0,0,0]` and
the symbol SHALL be marked `is_external: false` only if it has at least
one `defined_in` child in source.

#### Scenario: A real method's def_range is non-zero

- **WHEN** a method declared on line 10 columns 13–16 is indexed
- **THEN** its `symbol` frame contains `def_range: [10,13,10,16]`

#### Scenario: A synthetic root package may omit def_range

- **WHEN** the indexer emits the synthetic `cs:pkg/MyApp` package frame
- **THEN** the frame either omits `def_range` or sets it to `[0,0,0,0]`

### Requirement: Synthetic Root Package per Assembly

For each assembly built from source in the workspace, the producer SHALL
emit exactly one `symbol` frame with `kind: "package"`, `name` equal to
the assembly name, and no `parent`. Every top-level namespace (direct
child of the global namespace) declared in that assembly SHALL be
emitted with a `defined_in` edge whose `target` is the package's id.

#### Scenario: A solution with one project emits one package frame

- **WHEN** the indexer runs against a solution with a single project
  `MyApp.csproj` whose AssemblyName is `MyApp`
- **THEN** exactly one `symbol` frame with `kind: "package"` and
  `name: "MyApp"` is emitted

#### Scenario: Top-level namespaces link to the package

- **WHEN** the assembly `MyApp` declares the namespace `MyApp.Services`
- **THEN** an `edge` frame with `edge_kind: "defined_in"`, `source` set
  to the namespace's id, and `target` set to the package's id is emitted

### Requirement: Edge Coverage v1 — Narrow Set

The producer SHALL emit edges of these kinds: `defined_in`, `contains`,
`implements`, `overrides`, `calls`. Every call site within a method,
constructor, accessor, or lambda body SHALL produce one `calls` edge
whose `source` is the id of the enclosing fn/method/class and whose
`target` is the id of the called member.

#### Scenario: A method body emits a calls edge

- **WHEN** method `Foo.Bar` invokes `Other.Quux()` on line 12
- **THEN** an `edge` frame is emitted with `edge_kind: "calls"`, `source`
  equal to `Foo.Bar`'s id, `target` equal to `Other.Quux`'s id, and
  `range: [12,8,12,16]` (or the actual span)

#### Scenario: A class implementing an interface emits an implements edge

- **WHEN** class `Foo` declares `: IBar`
- **THEN** an `edge` frame with `edge_kind: "implements"`, `source`
  equal to `Foo`'s id and `target` equal to `IBar`'s id is emitted

#### Scenario: An override emits an overrides edge

- **WHEN** method `Foo.Bar` overrides `Base.Bar`
- **THEN** an `edge` frame with `edge_kind: "overrides"`, `source` equal
  to the override's id and `target` equal to the base method's id is
  emitted

### Requirement: Edge Coverage Expansion to Full Parity

After the v1 narrow set is working end-to-end, the producer SHALL also
emit edges of these kinds: `type_use`, `field_access` (carrying a
`field_op` property of `"read"` or `"write"`), `instantiates`,
`generic_constraint`, `imports`, `corresponds_to`. These SHALL share the
same envelope shape as v1 edges, with `source` and `target` numeric ids.

#### Scenario: Field write emits field_access with field_op write

- **WHEN** a method assigns to `this.count` on line 14
- **THEN** an `edge` frame with `edge_kind: "field_access"`,
  `field_op: "write"`, and a non-zero `range` is emitted

#### Scenario: Generic instantiation emits instantiates

- **WHEN** code constructs `List<string>`
- **THEN** an `edge` frame with `edge_kind: "instantiates"` whose
  `target` is the id of `string` is emitted

### Requirement: Locals Are Not Emitted as Symbol Records

The producer SHALL NOT emit `symbol` frames for local variables, lambda
parameters, loop variables, range variables, anonymous types, or any
symbol Roslyn would classify as local in scope. Walking into method and
lambda bodies for the purpose of computing call/reference edges is
required, but the source of every such edge SHALL be the enclosing
fn/method/class — never a local.

#### Scenario: A method-local variable produces no symbol frame

- **WHEN** method `Foo.Bar` declares `int x = 5;`
- **THEN** no `symbol` frame is emitted for `x`

#### Scenario: A call inside a lambda is attributed to the enclosing method

- **WHEN** `Foo.Bar` contains `xs.Where(x => Helper.Check(x))`
- **THEN** the `calls` edge for `Helper.Check` has `source` equal to
  `Foo.Bar`'s id, not the lambda's

### Requirement: Documentation Inline on Symbol Frames

The producer SHALL inline `signature_doc` and `documentation` strings on
the `symbol` frame itself when present, omitting them when absent. The
`documentation` string SHALL be **plain prose** — the producer SHALL normalize
the XML doc comment returned by Roslyn into human-readable text, stripping the
`<member>` envelope and all doc tags, keeping the text of prose elements
(`summary`, `remarks`, `returns`, `value`, `example`, `param`, `typeparam`),
rendering inline reference tags (`see cref`, `paramref`, …) as their bare names,
and decoding XML entities. A doc whose only content is `<inheritdoc/>` (no
inline prose) SHALL be treated as absent (no `documentation` string emitted).
The consumer SHALL split these out into a separate `SymbolDocsRecord` row only
for symbols where at least one of the two strings is non-empty.

#### Scenario: Symbol with docs gets one symbol_docs row of plain prose

- **WHEN** a class has an XML doc comment `<summary>Holds the order.</summary>`
- **THEN** its `symbol` frame contains `documentation` equal to the prose
  `Holds the order.` (no `<member>`, `<summary>`, or `name="…"` markup)
- **AND** the consumer writes one `SymbolDocsRecord` row for it

#### Scenario: Symbol without docs gets no symbol_docs row

- **WHEN** a class has no signature renderer output and no XML docs
- **THEN** neither `signature_doc` nor `documentation` appears on its
  `symbol` frame
- **AND** the consumer writes no `SymbolDocsRecord` row for it

#### Scenario: inheritdoc-only comment emits no documentation

- **WHEN** a member's only doc comment is `<inheritdoc/>`
- **THEN** no `documentation` string is emitted for it (it is treated as
  undocumented; the inherited doc is not resolved)

### Requirement: File Frames Carry Hex Content Hash

Every `file` frame SHALL include `path`, `is_test`, `is_external`, and
`content_hash`. The hash SHALL be a lowercase hex string of an xxh64 digest
of the file's UTF-8 bytes. Numeric (`u64`) representation MUST NOT be used
on the wire because it overflows JavaScript-safe integers and risks
implementation drift between languages.

#### Scenario: File hash is hex

- **WHEN** any `file` frame is emitted
- **THEN** its `content_hash` field matches `^[0-9a-f]{16}$`

### Requirement: Forward Reference Resolution

The producer SHALL ensure that every `id` referenced by an `edge` (as
`source` or `target`) has been introduced by a `symbol` or `file` frame
emitted earlier in the same stream. The producer MAY introduce an id
via a "stub" `symbol` frame carrying provisional values for `kind`,
`name`, and `display_name`, while omitting `parent`, `file`, and
`def_range` until later.

The consumer SHALL UPSERT every `symbol` frame keyed on `id`. Later
frames overwrite earlier fields in place. The consumer MUST NOT treat a
re-emission as a duplicate or constraint violation.

For symbols that are referenced but never declared in any walked
project (BCL types, NuGet packages, framework metadata), the producer
SHALL emit exactly one stub with `is_external: true`, and no upgrade
ever follows.

#### Scenario: Stub before edge

- **WHEN** the producer is about to emit an edge whose endpoints have
  not yet been declared
- **THEN** the producer emits a `symbol` frame for each unknown endpoint
  first, on the same stream

#### Scenario: Stub upgraded to full

- **WHEN** the producer emits a stub `symbol` frame with `id: 42` and
  `is_external: false` while walking a method body, then later walks the
  symbol's declaration
- **THEN** a second `symbol` frame with the same `id: 42` is emitted
  carrying complete `parent`, `file`, and `def_range` fields
- **AND** the consumer UPSERTs the second frame, overwriting the stub

#### Scenario: Symbol never declared (true external)

- **WHEN** an `id` is referenced by edges but only ever appears as a
  stub `symbol` frame with `is_external: true` (e.g., a NuGet-only type)
- **THEN** the stub row remains with `is_external: true` for the
  remainder of the run

### Requirement: Streaming Ingest with Batched Writes

The Rust consumer SHALL read stdin line-by-line, deserialize each line as
a frame, and accumulate records into a batch. The batch size SHALL be
configurable via CLI flag with a default of 10000 records. The consumer
SHALL flush a batch to SurrealDB when the size threshold is hit and on
receipt of the `end` frame.

#### Scenario: Batches flush at the threshold

- **WHEN** the configured batch threshold + 1 records have accumulated
  since the last flush
- **THEN** the consumer has issued an `INSERT`/`RELATE` round-trip
  containing the first <threshold> records

#### Scenario: End frame triggers final flush

- **WHEN** the `end` frame is read
- **THEN** any remaining records are flushed before the consumer process
  exits

### Requirement: Consumer CLI Flags

The consumer SHALL expose CLI flags to configure its behavior. Required
flags:
- `--db <path>` selecting the embedded SurrealDB directory (required).
- `--batch-size <int>` to override the default 10000-record batch
  threshold.
- `--reset-db` to wipe and recreate the database before ingesting.
- `--input <path>` to read JSONL from a file instead of stdin (default:
  stdin).
- `--quiet` to suppress progress output, and `--progress` (default on
  TTYs) to print a periodic counter to stderr.

#### Scenario: --batch-size overrides default

- **WHEN** the consumer is invoked with `--batch-size 1000`
- **THEN** flushes occur every 1000 records instead of every 10000

#### Scenario: --input reads from a file

- **WHEN** the consumer is invoked with `--input /tmp/idx.jsonl`
- **THEN** the consumer reads lines from `/tmp/idx.jsonl` rather than
  stdin

#### Scenario: --reset-db wipes existing state

- **WHEN** the consumer is invoked with `--reset-db` against a directory
  that already contains a database
- **THEN** the directory is removed and recreated before ingest begins

### Requirement: Producer Errors Are Reported as Frames

Producer-side errors and warnings SHALL be reported as `error` frames
inline in the JSONL stream, not only via stderr logging or via the
`EndFrame.stats.errors` counter. Each `error` frame carries:

- `severity`: `"error"` or `"warning"`
- `source`: free-form short subsystem identifier (e.g. `"msbuild"`,
  `"indexer"`, `"roslyn"`)
- `message`: one-line human description (multi-line messages flattened
  to single line with spaces)
- Optional `path` (workspace-relative), `range`
  (`[start_line, start_col, end_line, end_col]`), and `code` (vendor code
  like `"MSB1234"`).

The `EndFrame.stats.errors` counter SHALL equal the count of emitted
`error`-severity frames. Warnings do NOT bump the counter. The producer
SHALL continue running after a non-fatal error frame and SHALL still
emit an `end` frame.

#### Scenario: MSBuild failure becomes an error frame

- **WHEN** MSBuild reports a `WorkspaceDiagnosticKind.Failure` while
  loading a project
- **THEN** an `error` frame is emitted with `severity: "error"`,
  `source: "msbuild"`, the failure message, and the project's `path`

#### Scenario: Skipped non-C# project becomes a warning frame

- **WHEN** the indexer encounters a Visual Basic or F# project in a
  multi-language solution
- **THEN** a `warning`-severity error frame is emitted with
  `source: "indexer"` describing the skip
- **AND** `EndFrame.stats.errors` is NOT incremented

#### Scenario: End frame error count matches emitted error frames

- **WHEN** N `error`-severity frames have been emitted during a run
- **THEN** the `EndFrame.stats.errors` field equals N

### Requirement: B1 Tokenizer Probe Output

The change SHALL produce a Markdown file at
`scratch/b1-tokenizer-findings.md` recording the observed behavior of
SurrealDB's `class` tokenizer plus the `@N@` BM25 match operator on a
real indexed snapshot. The file SHALL include, at minimum: the queries
run, the row counts returned, and a one-paragraph explanation of why
`search_symbols("WebhookHandler")` returns 0.

#### Scenario: Findings file exists with concrete queries

- **WHEN** the change is complete
- **THEN** `scratch/b1-tokenizer-findings.md` exists
- **AND** the file contains at least the queries for `WebhookHandler`,
  `Webhook`, lowercased `webhookhandler`, and at least one `@1@` or `@@`
  variant, with their observed result counts

### Requirement: PackageFrame is a top-level wire frame

The wire format SHALL include a `PackageFrame` frame type with the
shape:

```
{ type: "package", id: Ref, name: string,
  version?: string, manager?: string, external?: boolean }
```

`name` is the package's logical identifier within its ecosystem
(`App.Trading.Risk`, `Newtonsoft.Json`, `serde`). `version` is the
package's published or workspace version string when the producer
knows it. `manager` is a short ecosystem label (`"nuget"`, `"cargo"`,
`"npm"`, `"go"`, `"pypi"`) when meaningful. `external` SHALL be `true`
for packages outside the workspace (BCL, third-party deps) and omitted
or `false` for workspace-local packages.

Producers SHALL emit a `PackageFrame` before any `SymbolFrame` or
`StubFrame` that references it via `pkg`. Producers SHALL intern
packages producer-side by `(name, version)` so that multi-target
compilations of the same package do not emit duplicate `PackageFrame`s
on the wire.

#### Scenario: PackageFrame precedes SymbolFrames that reference it

- **WHEN** a `SymbolFrame` carries `pkg: N`
- **THEN** a `PackageFrame` with `id: N` MUST have appeared earlier in
  the stream

#### Scenario: External packages have external: true

- **WHEN** the producer encounters a symbol from a system or
  third-party assembly (not declared in the workspace)
- **THEN** the `PackageFrame` representing that assembly MUST set
  `external: true`

### Requirement: StubFrame is the explicit minimal-info frame

The wire format SHALL include a `StubFrame` frame type:

```
{ type: "stub", id: Ref, kind: SymbolKind, name: string,
  key: string, pkg?: Ref }
```

A `StubFrame` carries the minimum a consumer needs to allocate a
short id and intern the symbol by `(key, pkg)`. `SymbolFrame` always
denotes a fully-known record; producers MUST NOT emit `SymbolFrame`
for symbols on which they have only partial information.

When a producer emits both a `StubFrame` and a subsequent `SymbolFrame`
for the same logical symbol, both frames MUST carry the same `id`. The
consumer relies on wire-id collision to recognize the upgrade.
Producers MAY emit a `StubFrame` and never follow it with a
`SymbolFrame` (this is the standard pattern for external symbols
whose definition is outside the workspace).

#### Scenario: Stub-then-full upgrade reuses id

- **WHEN** a producer emits a `StubFrame` with `id: 42` and later emits
  a `SymbolFrame` for the same logical symbol
- **THEN** the `SymbolFrame` MUST carry `id: 42`

#### Scenario: External symbol emits one StubFrame and no follow-up

- **WHEN** the producer encounters a reference to an external symbol
  (defined outside the workspace)
- **THEN** the producer MUST emit exactly one `StubFrame` for that
  symbol
- **AND** MUST NOT emit a `SymbolFrame` for the same `id`

### Requirement: Per-project walks may run concurrently

The producer SHALL be permitted to walk projects within a single
`kenn-dotnet` invocation concurrently. The maximum concurrency is
controlled by the `--max-parallelism` CLI flag and defaults to
`Environment.ProcessorCount`. A value of `1` SHALL produce a strictly
serial walk equivalent to the pre-parallel behavior.

`MSBuildWorkspace.OpenSolutionAsync` and the project-load phase SHALL
remain serial; concurrency applies after `LoadProjectsAsync` has
returned the project list.

#### Scenario: Default parallelism uses processor count

- **WHEN** the user runs `kenn-dotnet index` without
  `--max-parallelism`
- **THEN** project walks dispatch to a worker pool sized to
  `Environment.ProcessorCount`
- **AND** wall-clock runtime on a multi-project workspace is lower
  than the equivalent `--max-parallelism 1` run

#### Scenario: max-parallelism 1 is bit-stable serial fallback

- **WHEN** the user runs `kenn-dotnet index --max-parallelism 1`
- **THEN** project walks happen one at a time
- **AND** the resulting JSONL output is identical to the pre-parallel
  serial behavior (modulo non-deterministic content like timestamps
  in `meta`)

### Requirement: Concurrent emission preserves ordering invariants

The producer SHALL preserve the introduce-before-reference invariant
even under concurrent emission: every `Ref` referenced by a frame's
`source`, `target`, `parent`, `pkg`, or `file` field MUST have been
introduced (as a `PackageFrame`, `FileFrame`, `StubFrame`, or
`SymbolFrame`) earlier in the stream. Concurrency does not relax
this contract.

Frame lines are atomic: each frame is fully serialized to one
JSON-encoded line before the next emission begins. Frames from
different workers MAY interleave at the line level — there is no
required cross-worker ordering — but no frame line is ever split or
truncated.

The single `meta` frame SHALL be emitted before any worker starts;
the single `end` frame SHALL be emitted after every worker has
completed.

#### Scenario: Concurrent writes do not corrupt JSONL framing

- **WHEN** `kenn-dotnet` runs with `--max-parallelism N` for any `N
  > 1`
- **THEN** every output line parses as a valid JSON object
- **AND** the line discriminator `type` field is one of the wire
  format's defined values

#### Scenario: References resolve under interleaving

- **WHEN** the consumer ingests the parallel-emitted stream
- **THEN** every `EdgeFrame.source` and `EdgeFrame.target` resolves
  to a previously-introduced symbol or file id
- **AND** every `SymbolFrame.parent`, `pkg`, and `file` resolves to
  a previously-introduced id
- **AND** the consumer-side counts (symbols, defs, edges) match those
  of an equivalent `--max-parallelism 1` run on the same workspace

### Requirement: FileFrame carries file-level comment trivia

The producer SHALL emit on each in-source `FileFrame` an optional `doc` field: a JSON array of strings carrying the file's comment trivia in source order, **one entry per comment trivia token** (not a merged block). Entries SHALL be drawn from two slots: (1) the leading trivia of the compilation unit's first token (the file header), and (2) each namespace declaration's leading trivia. Each `SingleLineCommentTrivia` SHALL be its own entry; each `MultiLineCommentTrivia` SHALL be one entry preserved verbatim including internal newlines. Per-token granularity is required so the consumer can filter a license line without discarding an adjacent purpose line. The producer SHALL NOT filter, classify, or drop any comment (license-boilerplate filtering is a consumer concern). When a file has no such trivia, `doc` SHALL be omitted (or empty) and no doc is emitted.

When the first token of the compilation unit is itself the `namespace` keyword (no usings or types precede it), slots (1) and (2) reference the same leading trivia; the producer SHALL emit each comment token exactly once (deduplicated by trivia span) rather than twice.

The extraction SHALL read syntax trivia, NOT `GetDocumentationCommentXml()`, because plain `//` / `/* */` headers are not returned by the documentation API and namespace declarations return empty documentation XML regardless of any `///` present.

#### Scenario: File-header comments are captured as separate entries

- **WHEN** a C# file begins with consecutive `//` lines before its first `using`/`namespace`/type
- **THEN** the file's `FileFrame.doc` array contains one entry per `//` line, in order

#### Scenario: A multi-line block comment is one entry

- **WHEN** the file header is a single `/* … */` block spanning multiple lines
- **THEN** it appears as one `FileFrame.doc` entry with its internal newlines preserved

#### Scenario: Namespace-leading comment is captured

- **WHEN** a comment sits immediately above a `namespace` declaration (e.g. after the using directives)
- **THEN** the `FileFrame.doc` array contains it as an entry

#### Scenario: A comment above a namespace with no preceding usings is not double-counted

- **WHEN** a file's first token is the `namespace` keyword and a comment block precedes it
- **THEN** each comment token appears exactly once in `FileFrame.doc` (not duplicated across the first-token and namespace slots)

#### Scenario: A file with no comment trivia emits no doc

- **WHEN** a C# file has no file-header and no namespace-leading comments
- **THEN** `FileFrame.doc` is omitted or empty and the consumer writes no file_docs row

#### Scenario: Producer does not filter license headers

- **WHEN** a file begins with a copyright/license header
- **THEN** the producer still emits it verbatim in `FileFrame.doc` (filtering happens on the consumer)

### Requirement: Enum members are emitted as `enum_member`

The C# producer SHALL emit `enum_member` as the wire `SymbolKind` for a field whose containing type is an enum, rather than `const`. This adopts the shared wire's `enum_member` value so the consumer resolves it to `Kind::EnumMember`, matching the Rust and Go indexers. Non-enum constant fields SHALL continue to emit `const`.

#### Scenario: C# enum member classifies as enum_member

- **WHEN** the producer walks a member of a C# `enum`
- **THEN** its `SymbolFrame.kind` is `enum_member` (not `const`), and the consumer resolves it to `Kind::EnumMember`

#### Scenario: A non-enum const field is unchanged

- **WHEN** the producer walks a `const` field that is not an enum member
- **THEN** its `SymbolFrame.kind` remains `const` → `Kind::Constant`

### Requirement: The C# indexer emits `extends_type` for extension methods

The C# indexer SHALL emit an `extends_type` edge for every extension method (a
method where `IsExtensionMethod` holds), from the method to the type it extends.
The extended type SHALL be resolved from the method's first (`this`) parameter
type, normalized to its `OriginalDefinition` so a generic receiver
(`this IEnumerable<T>`) targets the open generic type. The method's existing
`defined_in` edge to its holder static class SHALL be unchanged, and call
resolution (`order.Foo()` → the holder declaration via `ReducedFrom`) SHALL be
unaffected.

#### Scenario: a simple extension method

- **WHEN** the indexer walks `static void Foo(this Order o)` in `OrderExtensions`
- **THEN** it emits `extends_type` from `Foo` to `Order`
- **AND** it still emits `defined_in` from `Foo` to `OrderExtensions`

#### Scenario: a generic receiver targets the open type

- **WHEN** the indexer walks `static T First<T>(this IEnumerable<T> xs)`
- **THEN** the `extends_type` target is the `IEnumerable<>` original definition,
  not a constructed `IEnumerable<SomeConcrete>`

#### Scenario: an ordinary parameter does not create the edge

- **WHEN** a non-extension method takes an `Order` parameter
- **THEN** no `extends_type` edge to `Order` is emitted (only the existing
  `type_use` for the parameter type)

#### Scenario: a receiver type from another assembly

- **WHEN** an extension method extends an external type (`this string`)
- **THEN** an `extends_type` edge is emitted to an external stub for that type,
  not dropped

### Requirement: symbol frames carry a body range for the whole declaration

A `symbol` frame SHALL carry an optional `body` range (4-int, 0-based, same
convention as the existing `def_range` name span) giving the full declaration
node span — the whole `class`/`method`/`property` body, including its leading
doc comment and attributes. It SHALL be sourced from the declaration syntax node
(`ISymbol.DeclaringSyntaxReferences[0].GetSyntax()` →
`RangeUtil.FromSyntaxNode(node)`), not from `ISymbol.Locations` (which is the
name identifier and drives the existing name span).

When a symbol has no declaring syntax (metadata-only / external), the `body`
field SHALL be omitted; ingest treats an absent `body` as a `0` def body extent
and `get_source` falls back to the name span.

#### Scenario: a method emits a body range spanning its declaration

- **WHEN** a C# method's name identifier is on file line 41 and its declaration
  (attributes through closing brace) spans file lines 39–58 (0-based 38–57)
- **THEN** the `symbol` frame's `def_range` MUST be the name span at line 41
- **AND** its `body` MUST be `[38, …, 57, …]` (0-based, the whole declaration)

#### Scenario: a metadata-only symbol omits the body range

- **WHEN** an external/metadata symbol has no declaring syntax reference
- **THEN** the `symbol` frame MUST omit `body`

