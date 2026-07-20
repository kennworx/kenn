## Why

An agent should be able to clone a repository it has never seen, index it, and
query it — to inspect the code and find ideas worth borrowing. The intended
shape:

```
git clone <url> ./tmp/repo         # the agent's job; kenn never clones
kenn init -w ./tmp/repo            # configure kenn for THAT repo
kenn index -w ./tmp/repo
kenn find symbol Foo -w ./tmp/repo
```

Most of this already works. `--workspace` is a global flag (`main.rs:36`) and
`resolve_workspace` (`main.rs:392`) gives an explicit path top priority over
`CLAUDE_PROJECT_DIR`, git-toplevel, and cwd. Every store path is anchored at
`source_root`, drivers pass `workspace.root()` to their sidecars, and the
embedder is a per-user daemon that never sees a workspace.

Three things block the workflow.

**1. `kenn init` writes a config that indexes nothing.** It emits a static
template (`crates/kenn-cli/assets/starter_kenn.toml`) in which every language is
`enabled = false` — confirmed at `language/rust.rs:73`, `typescript.rs:49`,
`go.rs:48`, `markdown.rs:72`, and every sibling. `kenn index` then refuses to run
(`any_language_enabled`, `cmd_index.rs:69`). In our own repos we papered over
this by hand-writing `kenn.toml` once. An agent on a fresh clone cannot.

This is not a foreign-repo bug. Every first-time user of `kenn init`, in their
own project, gets a config that does nothing. The clone workflow just made it
impossible to ignore.

**2. `init` cannot be re-run, and a foreign `kenn.toml` bricks every command.**
`init` is idempotent by never overwriting (`cmd_init.rs:21-26`), so it can never
correct a wrong config. Worse: `Config::load_or_default` (`config.rs:63`) errors
on a *present but invalid* file and runs at `main.rs:266`, before
`dispatch_command` at `main.rs:306`. `Config` is `deny_unknown_fields`. Clone any
repo that has adopted kenn — including kenn's own — and a schema mismatch makes
**every** command fail, `init` included. There is no way to init out of it.

**3. A missing toolchain is a dead end, not a degraded mode.** `preflight`
(`pipeline/api.rs:334`) already hard-fails with `MissingCli` when an enabled
language's command is absent — but only for languages already enabled, which on a
starter config is none. Meanwhile `[language.text]` (`language/text.rs`) is a
shipped generic text producer: chunked, FTS + embeddings, no tree-sitter, no
sidecar, no code execution. Nothing points a user at it.

## What Changes

- **`-w` short alias** for the existing `--workspace` global. No other short
  flags exist in the CLI today, so `-w` is free.

- **`kenn init` detects the workspace and writes a config that fits it.** A
  single recursive walk collects marker files, pruning the union of every
  language's `DEFAULT_EXCLUDES` (`language/*.rs`, all `pub const`) so a `go.mod`
  under `vendor/` or a `Cargo.toml` under a test fixture never counts. This
  mirrors what Go's driver already does (`driver/go.rs:47-62`, which walks and
  prunes) and catches monorepos that a root-only check would miss.

  Detection is a **new table**, not reuse: only Rust (`driver/rust.rs:50`, root
  `Cargo.toml`) and Go have marker-shaped discovery. Python keys on configured
  `targets`; TypeScript, C#, and Swift are `JsonlIndexer`s with no
  `discover_units` at all.

- **Availability is verified by running the tool, not by finding it.**
  `is_command_available` (`pipeline/api.rs:356`) only checks that a file exists.
  A Homebrew `rustup` shim on `PATH` satisfies that and then breaks
  `rust-analyzer scip`. `init` runs the command's version probe and treats a
  non-zero exit, a spawn failure, or a timeout as unavailable.

  | marker found | version probe | `init` writes |
  |---|---|---|
  | yes | ok | `enabled = true`; `command` omitted (the default already resolves on `PATH`) |
  | yes | fails | language disabled; extensions added to `[language.text] include` |
  | no | — | language absent from the config |

  Detected-but-unavailable languages degrade to the **text fallback** rather than
  to nothing: the repo is immediately searchable by FTS and semantic search, with
  no symbol graph and no foreign code executed. The degraded language's own
  `DEFAULT_EXCLUDES` are merged into `[language.text] excludes` — otherwise a
  vendored Go repo would chunk *and embed* every dependency, since text's
  defaults cover `node_modules`/`target` but not `vendor/**` or `**/testdata/**`,
  and user-supplied excludes *replace* the defaults rather than extend them.

- **`init` seeds `[tests] paths` when none are configured.** The field is
  authoritative with no built-in fallback (`tests_config.rs:8`) and it is live:
  `workflow.rs:205` feeds it to `Workspace::with_test_globs`, and `workflow.rs:226`
  passes it to the .NET driver as `--test-glob`. Yet `Config::default().tests.paths`
  is empty, so a detection-rendered config would leave a workspace where *nothing*
  is test code — the same silent loss as a full `--force` re-render, on the fresh
  path. (This repo's own `kenn.toml` has `[tests]` with `paths` commented out, so
  kenn's index marks nothing as test code today.)

  `init` therefore contributes test globs from each **enabled** language's table
  entry, and only when `tests.paths` is empty — never clobbering a user's list.
  Degraded languages contribute nothing, because the text producer hardcodes
  `test: false` (`text/ingest.rs:112`); their globs would be inert. Enabling a
  language later, once its indexer is installed, does not retroactively extend a
  non-empty `paths` — `init` reports the globs it would have added instead of
  editing a list the user may have curated.

- **`init` reports what it decided**, non-interactively — enabled, degraded, and
  absent languages, plus a per-driver install hint for each failing probe. `init`
  never prompts: it is step two of an agent's four-step script, and a prompt hangs
  it. (Precedent: `rollback --yes` documents `--yes` as *"Required in non-TTY
  contexts"*; this change avoids needing the flag at all.)

- **`init --force` merges rather than replaces.** It deserializes the existing
  config, swaps only the `language` field, and re-serializes — `Config` already
  derives `Serialize` (`config.rs:19`), so this needs no new dependency. A full
  re-render would silently drop `[tests] paths`, which is *authoritative with no
  built-in fallback* (`tests_config.rs:8`) — meaning nothing in the workspace
  would ever again count as test code. It would also discard `[layout]`,
  `[vectors]`, `[staleness]`, `[metrics]`, and this repo's own `max_threads`,
  `low_priority`, C# `projects`, and markdown `excludes`.

  Values survive; comments do not. `init --force` writes `kenn.toml.bak` first so
  the loss is visible and recoverable.

- **`init` survives an unparseable `kenn.toml`.** It warns, resolves the store
  layout against `Config::default()`, and — with `--force` — replaces the file
  (backing it up; a merge is impossible when the input cannot be parsed).
  Structurally this follows `completions` and `cc-hook`, which already
  short-circuit before workspace/config resolution (`main.rs:234-245`).

## Capabilities

### Added Capabilities

- `workspace-init`: workspace targeting (`-w`/`--workspace`) and the `kenn init`
  contract — detect languages by pruned marker walk, verify indexers by version
  probe, write a fitting `kenn.toml`, degrade to the text fallback, report
  decisions and install hints, merge on `--force`, and remain runnable against a
  broken config.

  Note: `-w` is a global flag rather than an init concern, so it sits slightly
  awkwardly here. The alternative — `cli-query-surface` — is specifically about
  mirroring the MCP read tools, which is a worse fit.

## Impact

- **Config:** no schema change. `init` authors `[language.*]` blocks the config
  crate already accepts.
- **Dependencies:** none added. The typed merge uses the existing `toml = "1.1.2"`.
- **Behavior change:** `kenn init` in an existing project now writes an enabled
  config instead of an all-disabled template. This is the intended fix, and
  `init` still never overwrites without `--force`.
- **Store/schema:** none. No reindex required for existing workspaces.
- **Scope:** ~400–550 lines. Concentrated in `cmd_init.rs` plus a new detection
  module; the marker/probe/glob/hint table is new code, not a reuse of existing
  driver discovery.

## Deferred

Explicitly out of scope, in the order they were argued and set aside:

- **`command = "docker"`.** Discussed at length and deliberately deferred: docker
  is slower than a warm local toolchain, so it is a fallback, not a default. It
  also drags in a real path-space problem — drivers pass absolute host paths
  (`rust.rs:75-77`) and `ingest.rs:273` lets the indexer's own `project_root`
  override kenn's, so a container/host root mismatch makes every document fail
  `strip_prefix` and land in the `OutsideRoot` arm at `ingest.rs:284`, which is
  **silently skipped**. Result: empty index, exit 0. That silent skip should
  become a counted, reported outcome before any container work begins — it is
  independently a bug today.

- **A content-addressed SCIP cache** keyed on
  `(indexer identity, config_sig, HEAD)`. Note `compute_staleness_key`
  (`staleness.rs:117`) already computes exactly this and contains no path — it is
  only ever compared against a snapshot in the same directory. On a clean clone
  the dirty set is empty, so the key collapses to `(HEAD, config_sig)`. This is
  the single biggest lever on index build time for repeat clones, and it is sound
  only when the indexer's identity fully determines its output — which a pinned
  container image gives and a host toolchain does not.

- **Driver cwd coupling.** No driver calls `.current_dir()`; a relative `command`
  in `kenn.toml` (this repo uses `["build/kenn-ts"]`) resolves against the
  *process* cwd, not the workspace root. `-w` decouples those two for the first
  time. Not blocking here, because `init` omits `command` and lets it resolve on
  `PATH` — but it will bite anyone who hand-writes a relative command and uses `-w`.

- **Cross-repo vector cache.** `gc_vector_cache` (`generation.rs:186`) evicts
  whole generation directories and never the active one. Since all repos share a
  single `(model, dim, quant, recipe)` generation, pointing several workspaces at
  one shared `[vectors] location` would grow without bound. Finer-grained LRU
  must land before that is safe advice.
