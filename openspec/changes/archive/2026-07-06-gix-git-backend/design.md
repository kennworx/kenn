# Design

## D1 — One internal git module, four operations

All git access collapses to a small `kenn-store` module (e.g. `git.rs`) backed by
`gix`, exposing exactly what the call sites need — nothing more:

| kenn use today (subprocess) | gix equivalent |
|---|---|
| `git rev-parse HEAD` → HEAD sha (`staleness.rs`, `main.rs`) | `repo.head_id()` |
| `git status --porcelain` → tracked-modified paths (`staleness.rs`) | `repo.status(...)` iterator, filtered to tracked changes |
| `git worktree list --porcelain` → main worktree (`worktree.rs`) | `repo` common-dir → main work-dir; `repo.worktrees()` for linked |
| `git rev-parse` → repo/work-dir root (`main.rs`, `cmd_cc_hook.rs`, `canonicalize.rs`) | `repo.work_dir()` / `repo.git_dir()` |

The module returns the same shapes the call sites already consume (a HEAD string,
a sorted set of `(path, dirty-kind)`, a main-worktree `PathBuf`), so the diff at
each call site is mechanical. Non-git detection = "opening the repo fails" →
existing fallback (`None` / tree-fingerprint form) is preserved.

## D2 — Highest-risk port: `git status --porcelain` → `gix status`

The staleness key hashes **tracked files reported modified** and adds a **deletion
sentinel** for tracked deletions; it must **not** read untracked files (bounds
cost, ignores `node_modules/`). Porting this to `gix status` must preserve exactly
that set:

- Iterate index-vs-worktree status; keep entries whose status is modified / deleted
  / (typechange) for **tracked** paths; emit the deletion sentinel for deletes.
- Exclude untracked and ignored entries — do **not** enable the dirwalk's untracked
  collection beyond what's needed, so a huge untracked dir stays free.
- Verify the sorted dirty-path set and the sentinel match the current
  `git status --porcelain`-derived key byte-for-byte on a fixture repo (renames,
  deletes, mode changes, nested paths). This is the acceptance gate for the port.

`gix`'s status platform is newer than its ref reading; treat it as the part to
test hardest, not assume.

## D3 — Rejected alternative: harden the subprocess instead

We could keep `Command::new("git")` and harden it: resolve an absolute git path,
set `GIT_CONFIG_NOSYSTEM=1` / `GIT_CONFIG_GLOBAL=/dev/null`, force `-c core.quotepath=false`
and `-c safe.directory=*`, and pass an explicit env. That removes *some* config
side-effects but keeps the external-binary + `PATH` + spawn-cost dependency and the
porcelain-parsing surface. Chosen against because it treats symptoms; `gix` removes
the dependency class. Kept documented as the escape hatch if the build-weight cost
(below) proves unacceptable.

## D4 — Build weight is the real cost; feature-gate hard

kenn's release profile is `lto = true, codegen-units = 1` over a large graph, so a
fat new dependency is felt at build time and in binary size. Mitigation:

- Depend on `gix` with `default-features = false` and enable only refs/worktree/status;
  explicitly exclude network (`blocking-http-transport-*`), `blocking-network-client`,
  and any `gitoxide-core`/CLI features.
- Measure the delta: record `just build-cli` wall time and `./build/kenn` size before
  and after, and the sccache impact. If the delta is large, revisit D3.

## D5 — Open questions

- `gix` respects `.gitignore`/attributes itself; confirm its "tracked-modified"
  classification matches git's for edge cases kenn's key depends on (intent-to-add,
  assume-unchanged, submodules — likely irrelevant here but confirm).
- Some kenn call sites (`cmd_cc_hook.rs`) run in a hook context where opening a full
  `gix::Repository` per invocation may cost more than a `rev-parse`; measure and, if
  needed, use `gix`'s lower-level discovery rather than a full repo open.
