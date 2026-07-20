# kenn CLI

`kenn` is a single-binary developer tool that maintains a per-workspace
code structure index. It runs language indexers, normalizes
their output into a SurrealDB snapshot, and atomically swaps that snapshot
in for any reader (the future query API or MCP tool surface).

## Quickstart

```sh
# 1. Initialize the workspace. Creates `.kenn/` (runtime state) and a
#    starter `kenn.toml`. Idempotent — running again is a no-op.
kenn init

# 2. Run an indexer pass. Discovers units (e.g. `.sln` files for C#),
#    invokes the per-language indexer, normalizes its output into a
#    SurrealDB snapshot, and atomically flips
#    `.kenn/live` to point at it.
kenn index

# 3. Inspect the current state. Prints snapshot path, key counts, any
#    regression warnings from the last flip, and a `fallback: parent`
#    label when this workspace is reading the main repo's snapshot.
kenn status

# 4. Roll back to the previous snapshot if a build went bad.
kenn rollback
```

## Subcommands

| Command | Purpose |
|---|---|
| `init` | Create `.kenn/` and write a starter `kenn.toml`. Idempotent. |
| `index [--force] [--json]` | Run the indexer pipeline; skip on a matching git-aware staleness key unless `--force` is set. `--json` emits one JSON event per progress step. |
| `status [--json]` | Print current snapshot info, counts, warnings. |
| `rollback [--yes]` | Atomically flip `live` to the previous retained snapshot. Requires `--yes` in non-TTY contexts. |
| `mcp` | Speak MCP over stdio against the live snapshot. Workspace path comes from `--workspace`, then `$CLAUDE_PROJECT_DIR`, then `roots/list`, then `git rev-parse --show-toplevel`, then cwd — see "kenn mcp" below. |
| `analyze [--top-n N] [--max-depth N] [--min-cluster N]` | Read the live snapshot's aggregated graph and write the analysis report (`[index] report_path`, default `kenn_report.md`) — god-nodes split by live/test/external, anchored hierarchical Louvain, flat-Louvain cross-check. Falls back to in-memory recompute with a warning when the snapshot pre-dates the aggregate-graph artifact. |
| `server <start \| stop \| status>` | Manage the per-user kenn daemon (embeddings host; future home for inter-agent / hook-memory capabilities). Auto-spawned on first embedding need. See [docs/kenn/server.md](../../docs/kenn/server.md). **Multi-user hosts: read the data-isolation warning first.** |
| `embed` / `update` | Background embedding passes (incremental / model-swap). See [docs/kenn/embeddings.md](../../docs/kenn/embeddings.md) for the external-provider configuration. |

## `kenn analyze`

Reads the snapshot's pre-computed aggregated graph (`aggregate_nodes` +
`aggregate_edges` tables, persisted by `kenn index`) and renders the
analysis report (`[index] report_path`, default `kenn_report.md`) with:

- **Summary** — node / edge / weight totals, anchor count, flat-community count.
- **God Nodes — User (Live) / User (Tests) / System / External** — top-N
  by weighted degree per slice; `--top-n N` (default 20) controls list size.
- **Anchored Hierarchy** — one section per anchor (`Cargo.toml` /
  `package.json` / `go.mod` / `pyproject.toml` / `*.csproj` boundaries,
  with the symbol's `pkg` field winning over the path-prefix fallback).
  Inside each anchor: single-level Louvain on the induced subgraph;
  communities ≥ `--min-cluster N` (default 20) recurse with Louvain
  again, up to `--max-depth N` (default 4) levels. Per-community headers
  carry size, test-ratio %, and a `— test infra` tag at the 60% threshold.
- **Flat Communities (cross-check)** — single-level Louvain over the
  whole graph (ignoring anchors). Communities whose members span more
  than one anchor get a `— cross-anchor` flag, surfacing concerns that
  cut across packages.

On snapshots built by a kenn binary that pre-dates the aggregate-graph
artifact, `kenn analyze` recomputes the projection in memory and prints
a one-line warning suggesting `kenn index --force`. The report shape is
identical either way.

Global flags: `--workspace <path>`, `--config <path>` (defaults to
`<workspace>/kenn.toml`). For most subcommands `--workspace` falls back
to `git rev-parse --show-toplevel` and then cwd; for `kenn mcp` the
chain is richer — see below.

## `kenn mcp`

Speaks MCP over stdio against the live snapshot. Designed to be
launched by an MCP host (Claude Code, Cursor, Zed) rather than a
human, but works either way.

**Workspace-resolution chain** (mcp-roots-discovery):

1. `--workspace <path>` flag if provided. Permanent for this server's
   lifetime — blocks all post-handshake rebinds.
2. `CLAUDE_PROJECT_DIR` env var if set to an existing directory.
   Claude Code sets this on every MCP subprocess at spawn time
   (verified via the `debug_env` MCP tool against Claude Code
   2.1.148). Other hosts don't.
3. `roots/list` request issued to the client after the MCP
   `initialize` handshake, when the client declared the `roots`
   capability. If the result differs from the tentative bind, kenn
   rebinds atomically (current snapshot stays serving until the
   recovery pipeline takes the new workspace to `Ready`).
4. `git rev-parse --show-toplevel` from the launching cwd.
5. cwd as a last resort.

The startup log records which source won, with a `reason` field
when steps 4 or 5 fired (so "kenn auto-launched and bound to the
wrong place" is a one-line diagnostic):

```
kenn-mcp: workspace discovery source=claude-project-dir path=/home/user/proj
kenn-mcp: workspace discovery source=git-toplevel path=/home/user/proj reason=no-claude-project-dir
```

**MCP host compatibility** (as of 2026-05):

| Host | `CLAUDE_PROJECT_DIR` | `roots/list` | `notifications/roots/list_changed` |
|---|---|---|---|
| Claude Code 2.1.x | yes | yes | **no** (anthropics/claude-code#31893) |
| Cursor | no | yes | yes |
| Zed (via ACP) | no | yes | yes |

For Claude Code, step 2 wins immediately at spawn — no wasted
indexing. For Cursor / Zed, step 4 or 5 binds tentatively before
the handshake; step 3 rebinds post-handshake if the host's
workspace differs.

**Single-root constraint**: when `roots/list` returns more than one
root, kenn binds to the first `file://` URI and logs the rest as
ignored. Multi-root indexing (one snapshot covering N roots) is out
of scope for this version; if the wrong root is picked, override
with `--workspace`.

**Debug tool**: every `kenn mcp` build (release or debug) exposes a
`debug_env` MCP tool that returns the subprocess's pid, cwd, and a
filtered env snapshot (`CLAUDE_*`, `CLAUDECODE`, `MCP_*`,
`AI_AGENT`, `XDG_*`, `HOME`). Use it to verify what env vars your
host actually passes — the only reliable way to check, since the
launching shell's env is not what the spawned subprocess sees.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success or skip (e.g., staleness match) |
| 1 | Generic error |
| 2 | Usage error (clap; bad flag, missing argument) |
| 3 | Workspace not identified |
| 4 | Lock contention (another `index` is running) |
| 5 | Indexer reported `Failed` |

## Configuration

`kenn init` writes a fully commented `kenn.toml` at the workspace root. Every section
is optional and falls back to the documented defaults if omitted. See the
generated file for the canonical reference; the keys mirror design D13 of
the `indexed-store-and-lifecycle` proposal.

## Storage layout

`.kenn/` lives at the workspace root.

```
.kenn/
├── live → snapshots/<timestamp>/   # symlink, atomically replaced on flip
├── snapshots/
│   ├── 2026-05-01T12-30-00Z/       # immutable RocksDB-backed SurrealDB
│   │   ├── meta.json               # counts + staleness key + warnings
│   │   └── ...                     # rocksdb files
│   └── 2026-05-01T15-45-00Z/       # current (live points here)
├── runs/
│   └── run-<unix-secs>/
│       └── report.json             # per-unit indexer reports (kept across GC)
└── index.lock                      # exclusive flock guarding writers
```

## Worktree fallback

When this workspace lacks a local `.kenn/live`, queries automatically
fall back to the main worktree's snapshot in read-only mode. This makes a
freshly-created `git worktree add` feature branch usable immediately, without
waiting for its own index. The worktree's own indexer always writes locally
and never touches the parent repo (no parent locks, no parent writes).

## Empirical anchors

Indexing is not free; the cost depends on workspace size. As a rough
shape on a recent laptop: a small C# sample (~10k LoC) takes ~10 s and
~5 MB of snapshot; a typical mid-size monorepo (~300k LoC) takes two to
three minutes and ~200 MB; a million-LoC workspace is in the five-minute
ballpark. Snapshot size scales with symbol+edge count, not LoC directly.

Reindexing is not on every change: the git-aware staleness check skips runs
when `(HEAD, sorted (path, xxhash))` matches the live snapshot's recorded
key. Branch switches with no edits, and explicit `kenn index` after a
true edit, are the dominant invalidation events.

## Architecture

See [docs/kenn/store-architecture.md](../../docs/kenn/store-architecture.md)
for the lifecycle state machine, atomic-flip semantics, and worktree
fallback flow.
