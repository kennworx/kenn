## ADDED Requirements

### Requirement: JSONL Wire Format

The dotnet-stream-indexer SHALL emit a sequence of newline-delimited JSON
frames on stdout. Each frame MUST be a single JSON object on a single line,
ending with `\n`, and MUST carry a `type` field that names the frame's
shape.

The frame types are: `meta`, `file`, `symbol`, `partial_def`, `edge`,
`end`. Producers SHALL emit exactly one `meta` frame as the first frame,
exactly one `end` frame as the last frame, and any number of the others in
between, in any order subject to the cross-reference rules below.

Cross-references between frames SHALL use string `pub_id` values only.
Numeric `short_id`s MUST NOT appear on the wire — they are a consumer-side
internal.

#### Scenario: First frame is meta, last frame is end

- **WHEN** the indexer runs against a non-empty workspace
- **THEN** the first line of stdout deserializes to a `meta` frame
- **AND** the last line of stdout deserializes to an `end` frame

#### Scenario: Each frame is one line of JSON

- **WHEN** the indexer emits any frame
- **THEN** the frame is a single line of JSON terminated by `\n`

#### Scenario: Cross-references use pub_id strings

- **WHEN** an `edge` frame references its source or target symbol
- **THEN** it uses `source_pub_id` and `target_pub_id` string fields
- **AND** no field on any frame contains a numeric `short_id`

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

### Requirement: Stable pub_id Scheme for C#

The producer SHALL assign each emitted symbol a stable string `pub_id` of
the form `cs:<native-syntax>` where `<native-syntax>` uniquely identifies
the symbol within the workspace. Method overloads SHALL be disambiguated
by parameter signature so two overloads of the same name receive distinct
`pub_id`s.

Synthetic root packages SHALL use the form `cs:pkg/<AssemblyName>`.

#### Scenario: Two overloads receive distinct pub_ids

- **WHEN** a class declares both `void Bar()` and `void Bar(int x)`
- **THEN** the two methods are emitted as two `symbol` frames with two
  different `pub_id` values

#### Scenario: Same symbol is emitted with the same pub_id across documents

- **WHEN** a partial class is split across two source files and both are
  indexed
- **THEN** both `symbol` frames for the class share one `pub_id`
- **AND** at least one `partial_def` frame is emitted to record the second
  declaration site

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
emit exactly one `symbol` frame with `kind: package`,
`pub_id: cs:pkg/<AssemblyName>`, and no parent. Every top-level namespace
(direct child of the global namespace) declared in that assembly SHALL be
emitted with a `defined_in` edge whose target is that synthetic package.

#### Scenario: A solution with one project emits one package frame

- **WHEN** the indexer runs against a solution with a single project
  `MyApp.csproj` whose AssemblyName is `MyApp`
- **THEN** exactly one `symbol` frame with `kind: package` and
  `pub_id: cs:pkg/MyApp` is emitted

#### Scenario: Top-level namespaces link to the package

- **WHEN** the assembly `MyApp` declares the namespace `MyApp.Services`
- **THEN** an `edge` frame with `edge_kind: defined_in`,
  `source_pub_id: cs:MyApp.Services`, and `target_pub_id: cs:pkg/MyApp` is
  emitted

### Requirement: Edge Coverage v1 — Narrow Set

The producer SHALL emit edges of these kinds: `defined_in`, `contains`,
`implements`, `overrides`, `calls`. Every call site within a method,
constructor, accessor, or lambda body SHALL produce one `calls` edge whose
`source_pub_id` is the enclosing fn/method/class and whose `target_pub_id`
is the called member.

#### Scenario: A method body emits a calls edge

- **WHEN** method `Foo.Bar` invokes `Other.Quux()` on line 12
- **THEN** an `edge` frame is emitted with `edge_kind: calls`,
  `source_pub_id: cs:MyApp.Foo#Bar()`,
  `target_pub_id: cs:MyApp.Other#Quux()`, and `range: [12,8,12,16]` (or the
  actual span)

#### Scenario: A class implementing an interface emits an implements edge

- **WHEN** class `Foo` declares `: IBar`
- **THEN** an `edge` frame with `edge_kind: implements`,
  `source_pub_id: cs:MyApp.Foo`, `target_pub_id: cs:MyApp.IBar` is emitted

#### Scenario: An override emits an overrides edge

- **WHEN** method `Foo.Bar` overrides `Base.Bar`
- **THEN** an `edge` frame with `edge_kind: overrides`,
  `source_pub_id: cs:MyApp.Foo#Bar()`,
  `target_pub_id: cs:MyApp.Base#Bar()` is emitted

### Requirement: Edge Coverage Expansion to Full Parity

After the v1 narrow set is working end-to-end, the producer SHALL also emit
edges of these kinds: `type_use`, `field_access` (carrying a `field_op`
property of `read` or `write`), `instantiates`, `generic_constraint`,
`imports`, `corresponds_to`. These SHALL share the same envelope shape as
v1 edges and use `pub_id` strings for cross-references.

#### Scenario: Field write emits field_access with field_op write

- **WHEN** a method assigns to `this.count` on line 14
- **THEN** an `edge` frame with `edge_kind: field_access`,
  `field_op: write`, and a non-zero `range` is emitted

#### Scenario: Generic instantiation emits instantiates

- **WHEN** code constructs `List<string>`
- **THEN** an `edge` frame with `edge_kind: instantiates` whose target is
  the type-argument symbol's `pub_id` is emitted

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
- **THEN** the `calls` edge for `Helper.Check` has
  `source_pub_id: cs:MyApp.Foo#Bar(...)`, not the lambda

### Requirement: Documentation Inline on Symbol Frames

The producer SHALL inline `signature_doc` and `documentation` strings on
the `symbol` frame itself when present, omitting them when absent. The
consumer SHALL split these out into a separate `SymbolDocsRecord` row only
for symbols where at least one of the two strings is non-empty.

#### Scenario: Symbol with docs gets one symbol_docs row

- **WHEN** a class has an XML doc comment
- **THEN** its `symbol` frame contains both `signature_doc` and
  `documentation`
- **AND** the consumer writes one `SymbolDocsRecord` row for it

#### Scenario: Symbol without docs gets no symbol_docs row

- **WHEN** a class has no signature renderer output and no XML docs
- **THEN** neither `signature_doc` nor `documentation` appears on its
  `symbol` frame
- **AND** the consumer writes no `SymbolDocsRecord` row for it

### Requirement: File Frames Carry Hex Content Hash

Every `file` frame SHALL include `path`, `is_test`, `is_external`, and
`content_hash`. The hash SHALL be a lowercase hex string of an xxh64 digest
of the file's UTF-8 bytes. Numeric (`u64`) representation MUST NOT be used
on the wire because it overflows JavaScript-safe integers and risks
implementation drift between languages.

#### Scenario: File hash is hex

- **WHEN** any `file` frame is emitted
- **THEN** its `content_hash` field matches `^[0-9a-f]{16}$`

### Requirement: Forward Reference Resolution in the Consumer

The Rust consumer SHALL accept `edge` frames whose `source_pub_id` or
`target_pub_id` has not yet been seen as a `symbol` frame. On first sight
of any unknown `pub_id`, the consumer SHALL register a stub
`SymbolRecord` (with `is_external: true` and `def_range: [0,0,0,0]`) and
assign a `short_id`. When the real `symbol` frame later arrives, the
consumer SHALL update the existing row in place using the same
`short_id`, clearing `is_external` and populating real fields.

#### Scenario: Edge before symbol

- **WHEN** an `edge` frame references `cs:MyApp.Foo#Bar()` before the
  matching `symbol` frame is read
- **THEN** the consumer creates a stub row for `cs:MyApp.Foo#Bar()` with
  `is_external: true`
- **AND** when the real `symbol` frame arrives, the row is updated in place
  with `is_external: false` and the real `def_range`

#### Scenario: Symbol never arrives (true external)

- **WHEN** a `pub_id` is referenced by edges but no `symbol` frame for it
  is ever emitted (e.g., a NuGet-only type)
- **THEN** the stub row remains with `is_external: true`

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
