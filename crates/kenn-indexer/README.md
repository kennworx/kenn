# kenn-indexer

Producer side of the kenn vertical slice. Consumes per-language indexer
output (JSONL from `kenn-dotnet` for C#; SCIP from scip-typescript,
scip-python, scip-go, rust-analyzer for the others) and emits records
conforming to the [`kenn-model`](../kenn-model) schema.

## Architecture

```
+------------------+    +-----------------+    +----------------+
| LanguageDriver   |--->| indexer binary  |--->| .scip / JSONL  |
| (KennDotnet,     |    | (subprocess)    |    +-------+--------+
|  RustAnalyzer, …)|                                   |
+------------------+                                   v
                                              +----------------+
                                              | parse_scip OR  |
                                              | parse_jsonl    |
                                              | (streaming)    |
                                              +-------+--------+
                                                      v
                       +---------+    +-------------------------+
                       | merge   |<---| transform_document      |
                       | dedup + |    | + edge::derive_edges    |
                       | collisions   +-------------------------+
                       +----+----+
                            v
                +-------------------------+
                | BatchingSink<S>         |  <-- accumulates per-record
                |   (default 10k records) |      pushes, flushes batches
                +-----------+-------------+
                            v
                +-------------------------+
                | Sink                    |  <-- trait: write_batch(&RecordBatch)
                |  ├── VecSink (tests)    |
                |  └── SurrealdbSink      |  <-- lives in
                |      (storage crate)    |      indexed-store-and-lifecycle
                +-------------------------+
```

## Installation

C# is indexed by `kenn-dotnet`, a self-contained single-file binary
shipped with this repo. Build it with `just build-indexer-dotnet` and
point `[language.csharp] kenn_dotnet_path` at the resulting
`./build/kenn-dotnet`. End users only need a .NET SDK on PATH (any of
8/9/10) — the runtime is bundled.

## Sample `kenn.toml`

```toml
[workspace]
root = "."

[exclude]
globs = ["vendor/**", "third_party/**"]

[language.csharp]
enabled = true
# kenn_dotnet_path = "/usr/local/bin/kenn-dotnet"
# projects = ["MyApp.sln"]    # restrict to specific .sln/.csproj; default: auto-discover
provision_directory_build_props = false

[ingest]
batch_size = 10000  # records per sink batch (default)

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

## Usage

```sh
kenn index                       # workspace = cwd, default config
kenn index --workspace /path     # explicit workspace
kenn index --config foo.toml     # explicit config
```

Records flow into whatever `Sink` impl the caller wires up. The storage crate
(`indexed-store-and-lifecycle`) provides `SurrealdbSink`, which streams
directly into an embedded SurrealDB. The CLI writes a per-run JSON summary
of `RunReport`s under `./.kenn/runs/` for observability.

## The `Directory.Build.props` caveat

`kenn-dotnet` uses MSBuildWorkspace which on some workspaces emits
NuGet-vulnerability errors that abort indexing. The fix is a top-level
`Directory.Build.props` that demotes those vulnerabilities to warnings.
`kenn` will provision one for you, but only when:

1. `provision_directory_build_props = true` in `kenn.toml`, AND
2. `--auto-fix-directory-build-props` is passed on the CLI (or
   interactive consent), AND
3. No `Directory.Build.props` already exists at the workspace root.

We never modify an existing file.

## Empirical anchors

Memory is bounded by per-document state during ingest; streaming SCIP
and JSONL parsing means peak RSS scales with the largest single
Document, not the whole stream. Wall-clock cost is roughly linear in
the number of source files MSBuildWorkspace has to load — typical
~300k-LoC C# repos finish in two to three minutes on a recent laptop,
small samples in well under a minute.
