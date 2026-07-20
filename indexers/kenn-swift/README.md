# kenn-swift

Streaming Swift indexer. Reads a SwiftPM package's compiler **index store**
(libIndexStore, via [`swift-index-store`](https://github.com/MobileNativeFoundation/swift-index-store))
and emits kenn JSONL frames on stdout — the Swift twin of `kenn-dotnet`.

There is no SCIP indexer for Swift; the compiler instead emits an index store as
a byproduct of `swift build` ("indexing while building"). kenn-swift builds the
package (unless `--skip-build`), reads the store, and translates its symbols and
relations into the wire (`../frames.ts`).

## Run

```sh
swift build
.build/debug/kenn-swift index --workspace /path/to/pkg --projects /path/to/pkg/Package.swift
# JSONL on stdout; build logs + diagnostics on stderr.
```

## Build (release, for kenn)

```sh
just build-indexer-swift     # → build/kenn-swift
```

The binary `dlopen`s `libIndexStore` from the active Swift toolchain at runtime
(shipped in Xcode / the Linux toolchain), so a Swift toolchain must be present —
which any Swift package being indexed already requires.

## Flags

| flag | default | meaning |
|---|---|---|
| `--workspace <dir>` | cwd | workspace root (paths are emitted relative to it) |
| `--projects <Package.swift>...` | (discover under workspace) | explicit package manifests |
| `--skip-build` | false | don't run `swift build`; read an existing `.build/index/store` only |

## What it emits

- **Symbols** keyed by a readable, composed name (`sw:MyApp.Order.save(x:)`);
  the full Swift signature lives in the separate `sig` field. `protocol` →
  `interface`, actors → `class`, accessors are folded away.
- **Extensions** are not nodes: an extension's members are attributed to the
  type they extend (collapsed onto one canonical node), carrying their own
  file/line.
- **Edges**: `calls`, `implements` (conformance + inheritance), `overrides`,
  resolved from index-store relations.

## Scope

SwiftPM only. Xcode projects (`.xcodeproj` / DerivedData) are out of scope.

## Tests

```sh
just test-indexer-swift   # builds Fixtures/SampleApp, indexes it, asserts the graph
```
