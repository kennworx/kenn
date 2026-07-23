## Context

Publishing a run ends by repointing `live` at the new run directory. Today that
is a POSIX symlink flipped by `rename(2)`, which gives two properties every
reader depends on:

- **atomic replace** — a concurrent reader sees the old target or the new one,
  never a missing or half-written pointer;
- **relative target** — the store stays relocatable.

`lifecycle/tests.rs` locks the first property in with a concurrent-reader test:
a thread calling `read_link` every 100ms across a flip, asserting no call errors.

Windows can provide neither through symlinks without elevation. The
`#[cfg(not(unix))]` arm therefore returns an error, so `kenn index` cannot
complete there — and the spec section its comment cites was never written.

Resolution is already centralised, which bounds this change: one writer
(`atomic_flip_live`) and two readers (`Store::live_target`,
`Layout::live_target` — the same eight lines duplicated in two files). The other
13 `live_path()` call sites only need the *path*, and are untouched.

## Goals / Non-Goals

**Goals:**
- `kenn index`, `rollback`, and MCP startup work on Windows with no elevation
  and no Developer Mode.
- Keep atomic replace and relative targets on all platforms.
- One code path for the flip, not a per-platform pair — a `cfg` fork is what
  produced a branch that compiled and always failed.
- CI catches Windows breakage on PRs rather than at release time.

**Non-Goals:**
- Docker indexer runtime on Windows. The six published images are Linux-only.
  Windows users use local toolchains, or Docker Desktop with WSL2 where the
  existing Linux images work unchanged. Recorded as a documented limitation.
- Migrating existing stores in place (see D3).
- Windows on ARM (`aarch64-pc-windows-msvc`).
- musl targets.

## Decisions

### D1 — `live` becomes a pointer file, on every platform

A regular file containing the relative target path. Not a symlink on POSIX and
a file on Windows: **one format everywhere**, because a per-platform format
means the concurrent-reader guarantee is only ever tested on one of them.

Atomic replace is preserved by the same write-temp-then-rename dance already
used: `rename` is atomic over an existing file on POSIX, and on Windows
`fs::rename` maps to `MoveFileExW` / `SetFileInformationByHandle` and replaces
an existing destination.

**Windows adds a failure mode POSIX does not have, and it is not corruption —
it is a failed flip.** Replacing a file another process holds open succeeds only
if that process opened it with `FILE_SHARE_DELETE`. Rust's own default share
mode is `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, so kenn's own
readers never block a flip. A third party that opens `live` without it —
antivirus, an editor, an Explorer preview — does, and `rename` returns a sharing
violation.

The pointer therefore never becomes inconsistent, but publishing can fail
transiently on Windows in a way it cannot on POSIX. See D6.

The cost, stated plainly: `ls -l live` no longer shows the target and `cd live`
no longer works. `cat live` and `kenn status` both answer the question, and the
file being plain text keeps it debuggable without kenn.

### D2 — Readers collapse to one implementation

`Store::live_target` and `Layout::live_target` are the same logic in two files.
Rather than editing both, `Store` delegates to `Layout`. Two copies of a
format-sensitive read is how one gets fixed and the other does not.

### D3 — No migration; both directions degrade to "reindex"

The project's stated policy is that the index format changes without migrations
(`README.md`: *"re-run `kenn index --force` after upgrading"*), so no dual-read
compatibility shim is written.

Worth checking that the failure is graceful rather than assuming it — and it is,
in both directions, because `live_target()` ends in `.ok()?`:

- **new binary, old store** — `read_to_string` on a symlink follows it to a
  directory and errors → `None` → `StartupDecision::Reindex { "no live run" }`.
- **old binary, new store** — `read_link` on a regular file errors → `None` →
  the same.

Neither path panics, corrupts, or silently serves a stale run. The user sees a
reindex, which is what the policy promises.

### D4 — `ancestor_device_id` compares volume prefixes on Windows

Its only caller is `same_filesystem`, used to decide whether the writer's temp
dir can be renamed onto the destination. It already returns `true` defensively
when the device id is unknown, on the reasoning that a misclassification
surfaces as a loud `EXDEV` at first write.

The direct Windows analogue of `st_dev` is the volume serial number, which is
only reachable through unstable std APIs (`windows_by_handle`) or a raw
`GetVolumeInformationByHandleW` call. Neither is worth it here: comparing the
path prefix (drive letter, or UNC `\\server\share`) after canonicalisation
answers the same question for every case this guards, using stable std only.

### D6 — The flip retries a bounded number of times on Windows

Because a third-party handle can make `rename` fail with a sharing violation
(D1), the flip retries on that error class — a few attempts with a short backoff
— before surfacing the failure.

Retrying is safe precisely because the operation is idempotent: the temp file
already holds the complete target, and nothing has been mutated yet. This is
NOT a retry that papers over an unknown failure; it is one that waits out a
known-transient external lock.

Failure after the retries SHALL be reported as a failed publish naming the
sharing violation, not swallowed. A silently unflipped `live` means the run
completed while `kenn status` still reports the previous one — precisely the
"succeeded but indexed nothing" shape this whole area keeps producing.

### D5 — Windows CI gate at `cargo check`, not full test

A `windows-2022` job running `cargo check --workspace --all-targets`. Check
rather than test, because the full suite needs an embedding model and Docker;
the failure mode being guarded against here is *"does not compile on Windows"*,
which is exactly what `check` catches, and it was the first thing CI found.

The target returns to `dist-workspace.toml` only after this gate is green and a
tagged release builds it.

## Risks / Trade-offs

**Losing `ls -l live` is a real ergonomic loss.** It is the one thing a symlink
does that a pointer file cannot. Accepted because the alternative is either
elevation (`symlink_dir`) or losing atomic replace (junctions), and atomicity is
load-bearing for concurrent readers while `ls -l` is convenience.

**The concurrent-reader test must be rewritten, and it is the test that matters
most here.** It currently asserts `read_link` never errors mid-flip. Rewritten
against the pointer file it must assert the same property *and* that no reader
ever observes a truncated or empty file — a failure mode a symlink could not
have. Mutation-check it by writing the file in place instead of via rename and
confirming it goes red.

**Windows remains partly unverifiable locally.** `rustc` segfaults under
`qemu-x86_64` on an arm64 host, so neither the Windows nor the amd64 build can
be exercised on the development machine. D5's CI gate is the substitute; treat
"green locally" as saying nothing about Windows.

**Pointer files are not symlinks to tools that follow symlinks.** Anything
outside kenn that walks the store expecting `live/` to be traversable will
break. `staleness.rs` uses `symlink_metadata` while walking, but for source
files rather than `live` — worth confirming during implementation rather than
assuming from this reading.
