## Why

kenn reads all git metadata by **spawning the `git` binary** (`Command::new("git")`
in `worktree.rs`, `staleness.rs`, `canonicalize.rs`, `main.rs`, `cmd_cc_hook.rs`)
and parsing porcelain output. This is not a shell dependency (`Command::new` execs
git directly, not via a shell), but it does depend on the external binary and its
environment, which is fragile:

- **`PATH` / environment inheritance.** kenn-mcp and kenn-server are frequently
  spawned by an editor or GUI whose `PATH` differs from the user's terminal, so
  `git` may be "installed" yet not found — silently degrading git-awareness.
- **git config side-effects.** `safe.directory` makes git *refuse* repos it deems
  foreign-owned (a real failure in containers / shared build boxes); `core.quotepath`
  and locale can mangle porcelain paths; hooks/aliases can interfere.
- **Cost & skew.** A process spawn per call (`git status` runs on every staleness
  check, a hot path) and porcelain-format differences across git versions.

Move git access in-process using **`gix` (gitoxide, `0.85.0`, pure-Rust)**. It
reads the repository directly — no external binary, no `PATH`, no `safe.directory`,
no porcelain parsing, no per-call spawn.

## What Changes

- Introduce a small internal **git abstraction** (in `kenn-store`) backed by `gix`,
  exposing exactly what kenn needs: HEAD commit id, tracked-modified file set,
  main-worktree path, and repo/work-dir root.
- Port every `Command::new("git")` call site to it: staleness
  (`git rev-parse HEAD` + `git status --porcelain`), worktree resolution
  (`git worktree list --porcelain`), and root/workspace resolution (`git rev-parse`).
- Remove the `git` subprocess dependency from library/runtime code. Observable
  behavior is unchanged; the git binary is no longer required on `PATH`.
- Depend on `gix` with **minimal features** — refs/HEAD, worktree, and status only;
  no network/HTTP/`blocking-*` — to bound the added build weight (see Design).

## Capabilities

### Modified Capabilities

- `workspace-staleness`: the git-form staleness key is computed from in-process git
  reads, not by invoking the `git` binary.

## Impact

- **Robustness:** git-awareness (staleness, worktree fallback, root resolution)
  works regardless of `PATH`, `safe.directory`, locale, or git version — including
  under editor/GUI-spawned MCP/server processes.
- **Speed:** no per-call process spawn on the staleness hot path.
- **Cost (the tradeoff):** `gix` is a net-new, sizable dependency; under kenn's
  `lto + codegen-units=1` release profile it adds build time and binary size.
  Feature-gating to the minimal surface is the mitigation; if the weight is
  unacceptable, the fallback is to *harden* the subprocess path instead (see Design
  D3) rather than migrate.
- **Behavior parity risk:** `gix status` (tracked-dirty detection) must reproduce
  `git status --porcelain` semantics for kenn's key — the highest-risk port
  (Design D2); non-git and `Unknown` forms are unchanged.
