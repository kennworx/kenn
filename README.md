# kenn

**A code graph for your codebase — and for the agents working on it.**

kenn indexes a workspace into a queryable graph of symbols, calls, types, and
scopes across six languages, then answers structural questions about it:
who calls this, what implements that, where is this used, what does this file
import.

```console
$ kenn find symbol JsonlSink
$ kenn list callers 'rs:kenn-indexer::transform_jsonl::stream::run'
$ kenn find "how are toolchain versions resolved"     # semantic
```

## Why not grep

Grep tells you *"not in the files I happened to search."* A graph enumerates
every edge in the workspace, so it can answer the question grep structurally
cannot:

> **"Nothing sets this."**

That distinction matters most to the thing reading your codebase fastest — an
AI agent. An agent that greps and finds nothing will confidently tell you code
is dead. kenn is built so it can tell you it *isn't*, and be right.

## Install

```console
brew install kennworx/tap/kenn
```

```console
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/kennworx/kenn/releases/latest/download/kenn-installer.sh | sh
```

Or from source (Rust 1.85+):

```console
git clone https://github.com/kennworx/kenn && cd kenn
just install               # → ~/.local/bin
```

## Quickstart

```console
cd your-project
kenn init                  # writes .kenn/ and a starter kenn.toml
kenn index                 # build the graph
kenn overview              # languages, packages, files, symbols, graph shape
```

Then ask it things:

| question | command |
|---|---|
| Where is `X` defined? | `kenn find symbol X` |
| Who calls `X`? | `kenn list callers <id>` |
| Where is `X` used? | `kenn find usages X` |
| Who implements `X`? | `kenn list implementers <id>` |
| Show me `X`'s source | `kenn get source <id>` |
| Anything about this concept? | `kenn find "<natural language>"` |

Output is [TOON](https://github.com/toon-format/spec) by default — compact and
skimmable. Add `--json` to pipe into `jq`.

## Languages

| language | indexer | toolchain |
|---|---|---|
| Rust | rust-analyzer | `rust-toolchain.toml` |
| TypeScript | kenn-ts (bundled) | — |
| C# | kenn-dotnet | `global.json` |
| Python | scip-python | `.python-version` |
| Go | scip-go | `go.mod` |
| Swift | kenn-swift | `Package.swift` |

Each language runs either against a **local toolchain** or in a **published
Docker image**, chosen per language in `kenn.toml`:

```toml
[language.go]
enabled = true
runtime = "docker"
```

The Docker images carry no language toolchain. They read the version your
repository pins and provision exactly that, on demand, into a shared cache
volume — because an image with a baked toolchain and a repo that pins a
different one don't fail loudly, they index zero files and exit 0. See
[`docker/README.md`](docker/README.md).

## For agents

kenn speaks **MCP**, so any MCP-capable agent can query the graph directly:

```console
kenn mcp                   # stdio MCP server over this workspace
```

The CLI verbs map 1:1 onto MCP tools (`find_symbol`, `list_callers`, …). For
Claude Code there's a plugin in [`claude-plugins/kenn`](claude-plugins/kenn)
with skills for navigation, tracing, and duplicate detection.

### Findings

Beyond code, kenn stores **findings** — durable, provenance-tracked conclusions
anchored to the files they describe:

```console
kenn findings add "The entrypoint must exec, not spawn — as PID 1 a
  spawn-and-wait parent never reaps orphaned grandchildren." \
  --tag directive --anchor docker/README.md
```

Anchored findings travel with the code. When a file changes, its findings are
flagged as drifted rather than silently rotting, so the next person — or agent
— sees that the ground truth moved.

## Semantic search

`kenn find <query>` runs vector search over code *and* findings, using a local
embedding model (EmbeddingGemma) with no network calls. Structural commands work
without it; only `kenn find <query>` and `kenn find similar` need embeddings.

```console
kenn doctor                # embedder health, dimension, latency, backend
```

## How it works

```
  source ──▶ indexers ──▶ JSONL ──▶ transform ──▶ SQLite
                                                  ├─ symbols + edges
                                                  ├─ FTS5   (lexical)
                                                  └─ vec0   (vectors)
```

Each snapshot is immutable; `live` is flipped atomically on success and
`kenn rollback` flips it back. Details in [`docs/`](docs/) —
[store architecture](docs/kenn/store-architecture.md),
[embeddings](docs/kenn/embeddings.md), [server](docs/kenn/server.md).

## Status

Pre-1.0 and moving. The index format changes without migrations — re-run
`kenn index --force` after upgrading.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
