## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: SymbolFrame fields and naming

`SymbolFrame` SHALL have the shape:

```
{ type: "symbol",
  id: Ref,
  pkg?: Ref,
  key: string,
  kind: SymbolKind,
  name: string,
  parent?: Ref,
  file?: Ref,
  range: [number, number, number, number],
  partial?: boolean,
  nargs?: number,
  targs?: number,
  test?: boolean,
  sig?: string,
  doc?: string }
```

`key` SHALL be a language-naked, intra-package path. The `<lang>:`
prefix and `<asm>/` segment that previously appeared in keys are no
longer emitted by producers. The consumer assembles `pub_id` as
`<lang_prefix>:<key>` using `MetaFrame.language`.

`range` is the identifier-span of the primary declaration site and is
required (it was previously `def_range?` and optional). Stubs (which
do not have a known range) use `StubFrame` instead.

Boolean flags use the `<flag>?: boolean` convention with default false
(omit when false). Renames from previous naming: `is_partial` →
`partial?`, `is_test` → `test?`, `args_arity` → `nargs?`,
`generic_arity` → `targs?`. The fields `is_external`, `is_stub`, and
`display_name` are removed.

`sig` (formerly `signature_doc`) is bare signature text without code
fences. Code-fence wrapping is presentation-layer responsibility.

`doc` is the renamed `documentation` field.

#### Scenario: Symbol key carries no language or assembly prefix

- **WHEN** the producer emits a `SymbolFrame` for a method
  `Save(int)` on `Models.Order` in package `Web`
- **THEN** the frame's `key` MUST equal `"Models.Order#Save(int)"`
- **AND** the frame's `pkg` MUST resolve to a `PackageFrame` with
  `name: "Web"`

#### Scenario: Boolean flags omit when false

- **WHEN** the producer emits a `SymbolFrame` for a non-partial,
  non-test symbol
- **THEN** the JSON output MUST NOT contain a `partial` key
- **AND** MUST NOT contain a `test` key

#### Scenario: sig is emitted as bare text

- **WHEN** the producer emits a `SymbolFrame` with a known signature
- **THEN** `sig` MUST contain the bare signature line
  (e.g., `"public Task<int> Save(string name, int age)"`)
- **AND** MUST NOT contain a triple-backtick fence or language hint

### Requirement: Partials emit one SymbolFrame per declaration site

Producers SHALL emit one `SymbolFrame` per declaration site of a
partial symbol (C# `partial class`, Rust `impl` blocks, etc.), each
with `partial: true` and distinct wire ids sharing the same `(key,
pkg)`. The consumer's dedup logic on `(key, pkg)` collapse appends
additional declaration sites without dropping edges from each
declaration's wire id.

The `PartialDefFrame` frame type previously used to record additional
declaration sites is removed; producers MUST NOT emit it and consumers
MUST NOT expect it.

#### Scenario: Three-file partial class produces three SymbolFrames

- **WHEN** the workspace contains `Models.Order` declared `partial`
  across three files
- **THEN** the producer MUST emit three `SymbolFrame`s with
  `partial: true`, distinct `id`s, and identical `key` and `pkg`

## REMOVED Requirements

### Requirement: PartialDefFrame for additional declaration sites

**Reason:** Replaced by emitting one `SymbolFrame` per declaration
site with `partial: true` and distinct wire ids; the consumer's
dedup-with-append branch on `partial: true` records additional
declaration sites without a separate frame type.

**Migration:** Producers stop emitting `PartialDefFrame`. Consumers
stop handling it. Reindex (no data migration).

### Requirement: SymbolFrame.is_external

**Reason:** `external` lives on `PackageFrame` only. The consumer
denormalizes the symbol's external status from
`packages[pkg].external` at insert time.

**Migration:** Producers stop emitting `is_external`. Consumers
denormalize at insert.

### Requirement: SymbolFrame.display_name

**Reason:** Redundant with `name` and `sig`. Consumers render display
forms from those fields.

**Migration:** Producers stop emitting `display_name`. Consumers
that previously read it render from `name` and `sig`.

### Requirement: SymbolKind variant "package"

**Reason:** Packages are now a separate top-level frame type
(`PackageFrame`). The synthetic `kind: "package"` Symbol-root per
assembly was dead weight.

**Migration:** Producers stop emitting `kind: "package"` Symbols.
Consumers stop expecting them.
