## Why

`kenn.exe` runs natively on Windows (v0.2.0), but every indexer is declined
there: `kenn init --docker` refuses with "unsupported on Windows", and kenn ships
no native indexer binary. Yet the docker runtime is platform-neutral except for
**one** gap — it bind-mounts the workspace at its **own absolute path** (the
POSIX `SamePath` mount), which cannot work for a `C:\…` path inside a Linux
container. Docker Desktop runs the published Linux images fine; only the mount
strategy is missing — `docker.rs` already names `MountStrategy::Translate` in
comments as the documented seam for it (the enum variant itself is not yet
written). Filling that seam makes native `kenn.exe` drive Docker Desktop and
index **all six languages — including Swift**, which no native binary could.

## What Changes

- **Implement `MountStrategy::Translate`**: mount the Windows workspace at `/work`
  with `-w /work`, translate the workspace-root argument the drivers pass (host
  `C:\…` → `/work`), and drop `--user` on Windows. Two `/work`-constant
  substitutions are needed — the input root arg AND the indexer's reported
  `metadata.project_root` at ingest (else `canonicalize` drops every record as
  outside the host workspace root and the index is silently empty) — but **no
  per-file path rewrite**, since each document's path is already
  workspace-relative (`source-data-model`).
- **Reverse the blanket Windows decline**: `containerize_decision` probes the
  daemon on Windows and containerizes when Docker Desktop is runnable, uniform
  with POSIX hosts, instead of refusing unconditionally.
- **Update the decline message** from "unsupported on Windows" to "requires
  Docker Desktop" — decline only when the daemon is absent, as on any host.
- **BREAKING (spec-level)**: supersedes windows-support's requirement *"The Docker
  indexer runtime is unsupported on Windows"*. That requirement was an explicit
  deferral ("Windows path translation is a separate follow-on change"); this is
  that follow-on.

## Capabilities

### New Capabilities
<!-- none — extends the existing docker runtime -->

### Modified Capabilities
- `docker-indexer-runtime`: the container-invocation synthesis gains the Windows
  translated mount, and `kenn init --docker` selects the runtime on Windows when
  Docker Desktop is present rather than declining. This supersedes the
  unsupported-on-Windows requirement introduced by the not-yet-archived
  `windows-support` change (in its `windows-platform-support` capability), which
  this change's archive must REMOVE.

## Impact

- `crates/kenn-indexer/src/docker.rs` — add the `Translate` variant and its
  `docker run` shape (`/work` mount, no `--user`).
- The driver arg-formation that appends the workspace root — jsonl driver and
  SCIP driver — routes `/work` under `Translate` (design Decision 2).
- `crates/kenn-cli/src/cmd_init.rs` — `containerize_decision` probes the daemon on
  Windows and containerizes; the decline message changes.
- **Sequencing**: depends on `windows-support` archiving first (it introduced the
  requirement this reverses). Land after it, or fold the removal in.
- **No data-format / reindex change** — wire paths stay workspace-relative; a
  workspace indexed via docker on Windows stores the same relative, canonical
  paths as any other host.
- **Not touched**: native sidecar binaries (rejected — indexers run in docker);
  local/non-docker native Windows indexing.
