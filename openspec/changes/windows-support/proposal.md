## Why

kenn does not run on Windows, and the gap is larger than "we haven't built it
there".

The v0.1.0 release run added `x86_64-pc-windows-msvc` and failed
(GitHub run 29749477868). Two distinct blockers, in `kenn-store`:

1. **It does not compile.** `ancestor_device_id`
   (`crates/kenn-store/src/layout/resolve.rs:63`) uses
   `std::os::unix::fs::MetadataExt` and `meta.dev()` with no cfg gate —
   `E0433: cannot find 'unix' in 'os'` and
   `E0599: no method named 'dev'`.

2. **Even once it compiles, every index fails.** `atomic_flip_live`
   (`crates/kenn-store/src/lifecycle/atomic.rs:19-28`) has a
   `#[cfg(not(unix))]` branch that unconditionally returns
   `Err("atomic flip is POSIX-only in v1")`. Publishing a run ends by
   flipping the `live` pointer, so on Windows `kenn index` cannot
   succeed at all.

Blocker 2 is the real one, and it is a **requirement**, not an oversight:
`store-layout` specifies `live` as a symlink whose target SHALL be a relative
path. Windows cannot satisfy that unprivileged — `symlink_dir` requires
Administrator or Developer Mode.

The code comment points at `index-lifecycle §Atomic flip portability` for the
rationale. **That spec does not exist**; there is no Windows coverage anywhere
in `openspec/specs/`. So the deferral was never actually recorded, and the
first person to try Windows found out by watching CI fail.

Why now: Windows is a stated target platform, and `x86_64-pc-windows-msvc` is
currently listed in `dist-workspace.toml`. Every tag will fail the whole release
— including the Homebrew publish, which runs after the full matrix — until this
lands or the target is removed.

## What Changes

**The `live` pointer stops being a symlink and becomes a small pointer file**
holding the target run's relative path. This is the only option that is both
unprivileged and atomically replaceable on all three platforms:

| option | unprivileged | atomic replace | verdict |
|---|---|---|---|
| `symlink_dir` | no — admin/Developer Mode | yes | unacceptable for most users |
| directory junction | yes | no | loses the invariant readers depend on |
| pointer file | yes | yes (`rename` / `ReplaceFile`) | chosen |

The cost is real and worth stating plainly: `live` stops being inspectable with
`ls -l` and following it stops working with `cd live`. The mitigation is that
`kenn status` already reports the resolved run, and the file is plain text —
`cat live` answers the same question.

**The surface is small.** A `kenn find usages live_path` returns 15 hits, but
that count is misleading in both directions: at least two are false positives
(`GodNodeFilter::db_name` and `build_god_records` reference the *string*
`"live"`, nothing to do with the pointer), and the rest mostly want the path
rather than its target. Grounding it on the actual format-sensitive
operations — every `read_link` and `symlink` call in the workspace — gives one
writer, two readers, and a handful of test sites:

- writer — `atomic_flip_live`
- readers — `Store::live_target` and `Layout::live_target` (the same
  `read_link` logic, duplicated in two files; this change collapses it)
- tests — `worktree.rs` (`symlink_metadata` ×2), `lifecycle/tests.rs`
  (the concurrent-reader test asserting `read_link` never errors mid-flip),
  and `layout/store.rs:165` (`assert!(store.live_path().is_symlink())`)

Verified, not assumed: **nothing joins a path onto `live_path()`** anywhere in
the workspace, so no caller treats `live/` as traversable. Had that been false,
the pointer file would have been the wrong design rather than a contained
change.

**Scope decisions taken here:**

- **Docker indexer runtime on Windows is DEFERRED, explicitly.** The six
  published images are Linux-only. Windows users get local toolchains, or
  Docker Desktop with WSL2 where the Linux images work unchanged. This change
  records that as a documented limitation rather than leaving it to be
  rediscovered.
- **`x86_64-pc-windows-msvc` is removed from `dist-workspace.toml` as the first
  task, not the last.** A binary that compiles, installs, and then fails on the
  user's first `kenn index` is worse than no binary. It returns only once the
  flip works and CI proves it.

## Capabilities

### New Capabilities
- `windows-platform-support`: what kenn guarantees on Windows — which
  subsystems work, which are explicitly unsupported (docker runtime), and the
  filesystem primitives the store may rely on across platforms.

### Modified Capabilities
- `store-layout`: `live` changes from a symlink to a pointer file. The
  requirement that its target "SHALL be a relative path" survives; the
  requirement that it is a *symlink* does not. Atomic-replace and
  concurrent-reader guarantees must be restated in platform-neutral terms.

## Impact

**Code**
- `crates/kenn-store/src/lifecycle/atomic.rs` — the writer; drop the
  `cfg(not(unix))` error branch
- `crates/kenn-store/src/layout/store.rs`, `layout/types.rs` — the two
  duplicated readers
- `crates/kenn-store/src/layout/resolve.rs` — `ancestor_device_id` needs a
  Windows implementation (volume serial number) or a documented fallback
- `crates/kenn-store/src/worktree.rs`, `lifecycle/tests.rs` — tests asserting
  symlink mechanics
- `dist-workspace.toml` — remove the Windows target now, restore it at the end

**On-disk format** — existing stores have a symlink at `live`. Readers must
handle both, or the change must state that a reindex is required. This is a
migration question the design has to answer, not an implementation detail.

**CI** — Windows is the one target not verifiable from a macOS or Linux
machine: the vendored llama.cpp build goes through cmake/MSVC and the
`cfg(not(unix))` paths only ever compile there. A `cargo check` job on
`windows-2022` should gate this, so the next failure is a PR failure rather
than a failed release.
