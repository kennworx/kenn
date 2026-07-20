# kenn — Claude Code plugin

Code intelligence for an indexed codebase: symbol search, call/type-graph
navigation, semantic search, and a provenance-tracked findings store. Driven
through the **`kenn` CLI**; the plugin ships skills that teach Claude Code when
and how to use it (plus an optional MCP server exposing the same operations as
tools).

## Install

kenn is a small toolchain: the **`kenn`** binary (query + orchestration, with a
bundled embedding model and SQLite) plus a per-language **indexer** that
`kenn index` shells out to — `kenn-dotnet` (C#), `kenn-ts` (TypeScript),
`kenn-swift` (Swift) — and `rust-analyzer` for Rust. Install `kenn` plus the
indexers for the languages you actually use; all are found on `PATH`.

### From this repo (recommended today)

```sh
just install                     # → ~/.local/bin
just install /usr/local/bin      # custom prefix (positional)
```

`just install` release-builds `kenn` (needs `cargo`) and each indexer whose
toolchain it finds — `kenn-dotnet` needs the .NET SDK, `kenn-ts` needs `bun`,
`kenn-swift` needs the Swift toolchain — skipping the rest and telling you.
Rust indexing additionally needs `rust-analyzer` on `PATH` — use a **Dec-2025 or
later** build (`brew install rust-analyzer`, or a current `rustup`), so
`kenn get source` returns whole items; the rustup-bundled build can lag and
older ones make `get source` return only the declaration line for Rust.

Just the core CLI (enough to *query* an existing index, not build one):

```sh
cargo install --path crates/kenn-cli
```

### As a Claude Code plugin (the skills)

```
/plugin marketplace add <path-or-repo>
/plugin install kenn@kenn
```

The plugin ships the skills (which drive the `kenn` CLI) plus the optional MCP
server; it still needs the `kenn` binary on `PATH` per above.

### Prebuilt binaries

A GitHub-release + Homebrew-tap path is planned but not yet available — it needs
a public repo to publish per-platform builds of all four binaries.

## What you get

- **Skills** — `kenn` (when/how to drive the CLI) plus the workflow skills
  (`recall`, `squeeze`, `reconcile`, `blast`, `trace`, `dup`, `audit`). These
  call the `kenn` CLI directly.
- **Optional MCP server `kenn`** — exposes the same code-graph + findings
  operations as tools, for hosts that prefer MCP. Not required by the skills.
- **Conversation capture hooks (preview)** — `SessionStart`,
  `UserPromptSubmit`, `PostToolUse` (Edit/Write/Read), and `SessionEnd`
  are wired to `kenn cc-hook ...` so Claude Code session activity lands
  in `.kenn/history/raw/<session_id>.jsonl`. Capture is silent and never
  interrupts a session. See [`docs/kenn/cc-hook.md`](../../docs/kenn/cc-hook.md)
  for what is captured and the trust boundary. If you already wired the
  same hooks via `kenn cc-hook install --write`, remove those entries
  from `~/.claude/settings.json` so records don't double up.

Run `kenn index` once to build the index (`kenn status` shows the live
snapshot); the skills read it from there.
