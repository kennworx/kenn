## ADDED Requirements

### Requirement: pub_id is a valid unquoted shell argument

kenn SHALL emit, for every code language (rust, typescript, csharp, python, go,
swift), a `pub_id` that is safe to pass as a single unquoted POSIX-shell argument.
The id MUST NOT contain any shell-hostile character — namely backtick, dollar,
parentheses, ampersand, pipe, semicolon, angle brackets, exclamation, the glob
metacharacters (asterisk, question mark, square brackets), braces, caret,
whitespace, single or double quote, or backslash — and MUST NOT begin with a hash,
tilde, or hyphen. Only the safe alphabet `A–Z a–z 0–9 . _ / : @ + , = ~ # -` may
appear, and the `rs:`/`ts:`/… prefix guarantees a safe first character.

#### Scenario: a TypeScript method id is shell-safe
- **WHEN** a TypeScript method whose descriptor includes a file path is indexed
  (today `` ts:`indexers/frames.ts`/walk(). ``)
- **THEN** its emitted `pub_id` contains no backtick and no `()`, e.g.
  `ts:indexers/frames.ts/walk+`, passable to `kenn get` unquoted

#### Scenario: a C# method-signature id is shell-safe
- **WHEN** a C# method with fully-qualified parameter types is indexed (today
  `cs:…IndexCommand#Run(Kenn…IndexOptions, Kenn…JsonlSink, …)`)
- **THEN** its emitted `pub_id` contains no `(`, `)`, `<`, `>`, or whitespace, and
  is a single shell token

#### Scenario: a Rust trait-impl id is shell-safe
- **WHEN** a Rust trait-impl / generic symbol is indexed (today
  `` rs:…::ExitCode::`From<ExitCodes>` ``)
- **THEN** its emitted `pub_id` contains no backtick, `<`, or `>`, e.g.
  `rs:…::ExitCode::From~ExitCodes`

### Requirement: callable and type structure use safe delimiters and constructor words

The id grammar SHALL carry callable and type structure with the safe delimiters `+`
(callable marker and parameter separator), `~` (opens/descends a constructor's
argument list), and `,` (sibling arguments in one list). Every type constructor
whose native syntax uses a shell-metacharacter — array `[]`, pointer `*`, nullable
`?`, tuple `()`, brace-form `{}`, union `|`, reference/`ref`/`out`/`in`, fn/closure —
SHALL be rendered as a reserved safe word plus `~` (e.g. `_array_`, `_ptr_`,
`_optional_`, `_tuple_`, `_ref_`, `_fn_`), never as its punctuation. The reserved
`_word_` form SHALL keep built-in formers distinct from user/library types of the same
name; a former that is itself a named library type renders by that name. SCIP backtick quoting SHALL NOT
appear in an emitted id. Operator-glyph names SHALL be mapped to safe words by a
general glyph→token rule (covering user-defined operators). The transform SHALL be
total: any hostile byte not otherwise handled SHALL be escaped by a safe residual
scheme, so no shell-hostile character can reach an emitted id. A function type SHALL
render its parameters as `+` slots and its return after `=`, the return always present
(void included). A value member SHALL render **bare** (no kind suffix), distinguished
from a same-named callable (`+`) or type (`#`) by the latter's marker.

#### Scenario: callable marker and params
- **WHEN** a method `Run` with parameters `A` and `B` is indexed
- **THEN** its id renders the call with `+` — `…#Run+A+B` — and `walk()` with no
  params renders `…walk+`

#### Scenario: generic arguments
- **WHEN** a generic type `HashMap<K, V>` and a nested `Result<Vec<u8>, Error>` are
  indexed
- **THEN** they render `HashMap~K,V` and `Result~Vec~u8,Error` (no `<`, `>`, or
  whitespace)

#### Scenario: array / tuple / nullable param types (glob & brace metachars)
- **WHEN** a method takes `int[]`, `(int, string)`, and `string?`
- **THEN** its id renders `_array_~int`, `_tuple_~int,string`, and `_optional_~string`
  — containing no `[`, `]`, `(`, `)`, `?`, or `{`, `}`

#### Scenario: function type carries its return after `=`
- **WHEN** a `fn(int) -> bool` and a `fn() -> void` are indexed as type references
- **THEN** they render `_fn_+int=bool` and `_fn_=void` (return present in both)

#### Scenario: a value member renders bare
- **WHEN** a field `config` and a method `config` coexist on type `Foo`
- **THEN** the field renders `Foo#config` (bare) and the method `Foo#config+`, and
  neither carries a trailing term marker

#### Scenario: operator-glyph name
- **WHEN** a Swift `static func +` or a user-defined operator `<*>` is indexed
- **THEN** its id uses a safe word (`op_add`, `op_lt_star_gt`), not a bare glyph

### Requirement: type references render by leaf name, qualified on contention

An external (referenced-package) type reference in a descriptor SHALL be spelled by
its leaf name; an internal type SHALL stay fully qualified. Disambiguation SHALL use
a per-language leaf registry: when two distinct external fully-qualified names
contend for the same leaf within one language's id-space, all contenders SHALL fall
back to the fully-qualified name. C# SHALL keep its parameter types as the overload
disambiguator; Swift SHALL key a function on base name plus argument labels, and SHALL
append the parameter type(s) when two overloads share an argument-label set.

#### Scenario: Swift type-only overloads stay distinct
- **WHEN** two Swift methods share the argument-label set `f(x:)` but differ in
  parameter type (`Int` vs `String`)
- **THEN** their emitted `pub_id`s differ (the parameter type is appended)

#### Scenario: external type is shortened
- **WHEN** a C# method takes `Microsoft.Extensions.Logging.ILoggerFactory` (external)
  and `Kenn.Dotnet.Cli.IndexOptions` (internal)
- **THEN** the external type renders as `ILoggerFactory` and the internal type stays
  `Kenn.Dotnet.Cli.IndexOptions`

#### Scenario: contended leaf falls back to FQN
- **WHEN** two distinct external types share a leaf name (e.g.
  `System.Timers.Timer` and `System.Threading.Timer`) within one language
- **THEN** both render fully-qualified, so their ids differ

### Requirement: the id stays unique and deterministic across runs

The transformation SHALL be injective (two distinct symbols map to two distinct
`pub_id`s) and deterministic (the same code yields byte-identical ids across runs,
independent of ingest/thread order). Reversibility is NOT required.

#### Scenario: distinct symbols keep distinct ids
- **WHEN** two symbols differ in any structurally-significant part of their descriptor
- **THEN** their emitted `pub_id`s differ

#### Scenario: re-indexing unchanged code is byte-identical
- **WHEN** the same unchanged code is indexed twice, including symbols whose external
  leaf names contend
- **THEN** every emitted `pub_id` is byte-identical across the two runs

### Requirement: no pub_id consumer regresses; callable classification unchanged

Changing the `pub_id` grammar SHALL NOT change any behavior that consumes a
`pub_id`. The callable-vs-reference edge classification (which reads the SCIP wire
symbol, not the `pub_id`) SHALL be unaffected. Every consumer that inspects `pub_id`
structure SHALL be inventoried and confirmed to parse only on characters the grammar
leaves literal; none SHALL be left matching a pre-change form.

#### Scenario: callable classification is unchanged
- **WHEN** the graph is rebuilt with the new ids
- **THEN** the callable-vs-reference edge classification produces the same result as
  before the change

#### Scenario: no pub_id consumer matches a stale form
- **WHEN** the id grammar changes
- **THEN** every `pub_id`-inspecting consumer (e.g. the markdown link-resolution
  suffix match) still resolves correctly

### Requirement: a per-language conformance test enforces shell-safety

The change SHALL add a test that scans every emitted `pub_id` — over fixtures
covering all six code languages — against the shell-hostile character set and fails
on any hit, naming the language + id. This test SHALL be the guard for the invariant.

#### Scenario: the conformance test catches a hostile id
- **WHEN** any indexer emits a `pub_id` containing a shell-hostile character
- **THEN** the conformance test fails and names the language + id

#### Scenario: all six languages pass on fixtures
- **WHEN** the fixture repos for rust, typescript, csharp, python, go, and swift are
  indexed
- **THEN** no emitted `pub_id` contains a shell-hostile character
