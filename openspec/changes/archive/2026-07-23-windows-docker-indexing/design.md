## Context

The docker runtime (`crates/kenn-indexer/src/docker.rs`) wraps a language's
`command` into a `docker run` invocation. It is **not** platform-gated — the only
Windows-specific behavior lives in `kenn init`'s `containerize_decision`, which
declines `--docker` on Windows. Two facts, both read from the code, define the
whole design:

1. **The mount is same-path.** `docker_launcher` emits `-v {root}:{root}` and
   `-w {root}`; the driver then appends the **absolute workspace root** as an
   argument, and the indexer emits paths relative to it. `MountStrategy` has one
   variant, `SamePath`; `Translate` (Windows `/work` + translation) is a
   documented stub. A `C:\…` host path cannot mount at its own path inside a
   Linux container — that is the entire gap.

2. **Per-document paths are workspace-relative, but `metadata.project_root` is
   absolute.** `source-data-model` requires `files.path` to be "workspace-relative
   and canonical", and each document's `relative_path` is relative. BUT the wire
   also carries `metadata.project_root` — an absolute `file://` URI — and
   `canonicalize.rs` combines `project_root + relative_path → absolute` and then
   **refuses anything outside the configured workspace root** (`OutsideRoot` →
   record dropped). On POSIX SamePath the container path *equals* the host path,
   so `project_root` reconciles with the workspace root for free — that is exactly
   why SamePath "needs no translation". On Windows Translate, `project_root` is
   `/work`, which is outside the host `C:\…\proj`, so **every record would be
   dropped and the index would be silently empty** unless `project_root` is
   reconciled to the host workspace root.

Together: Translate needs **two** substitutions of the fixed `/work` mount
constant — the input root argument, and the output `metadata.project_root` at
ingest — but it is still NOT a per-file path rewrite (each `relative_path` is
untouched).

## Goals / Non-Goals

**Goals:**
- Native `kenn.exe` on Windows drives Docker Desktop to index all six languages
  (TS, C#, rust, go, python, Swift) against the published Linux images.
- `kenn init --docker` on Windows behaves like a POSIX host: containerize when the
  daemon is runnable, error when it is not.

**Non-Goals:**
- Native sidecar indexer binaries (the rejected route).
- Local/non-docker native Windows indexing for any language.
- Changing the POSIX `SamePath` behavior — it stays exactly as is.

## Decisions

### Decision 1 — `MountStrategy::Translate` mounts at `/work`

Add the `Translate` variant. Its `docker run` differs from `SamePath` in:
`-v {win_root}:/work` (workspace at a fixed container path), `-w /work`, and **no
`--user`** (Decision 3). Everything else — the toolchain-cache volume, the
dependency/build caches, the image, the trailing command — is identical.

*Why `/work`, not a translated drive path (`/c/Users/…`)?* A fixed mount point
makes both substitutions (root arg + `project_root`) a constant, not a per-path
drive-letter computation, and matches the `/work` name the seam comment documents.

### Decision 2 — A threaded `ContainerMount` translates every absolute path arg (REVISED after the apply audit)

The apply-phase audit (task 2.1) disproved the original "one substitution" plan.
Each driver appends **multiple, driver-specific** absolute paths after the
launcher prefix, all derived from `workspace.root()`:

| Driver | Output | Absolute path args |
|---|---|---|
| kenn-ts (jsonl) | stdout | `--workspace <root>` |
| kenn-dotnet (jsonl) | stdout | `--workspace <root>`, `--projects <abs>`×N |
| kenn-swift (jsonl) | stdout | `--workspace <root>`, `--projects <abs>`×N |
| rust (scip) | `--output` file | `scip <unit.path>`, `--output <derived>` |
| go (scip) | `--output` file | `--module-root <unit.path>`, `--output <derived>` |
| python (scip) | `--output` file | `--cwd <root>`, `--target-only <unit.path>`, `--output <derived>` |

Because every one of these is `workspace.root()` or `workspace.root().join(rel)`,
translation is a **prefix swap**: `host_root` → `/work`. Introduce a
`ContainerMount { host_root, container_root: "/work" }`, present only under
docker+`Translate`, and apply `mount.to_container(path)` at each path-arg site in
each driver. `docker_launcher` cannot own this (the args are appended by the
driver, after the launcher prefix, and differ per driver), so the mount is
**threaded into the driver layer** alongside the existing `command`.

*Derived-root reachability (re-examined in apply):* the SCIP `--output` file is
`<source_root>/.kenn/local/scip-*.scip` (`Layout::default_for`), i.e. **inside
the workspace root**, so the `/work` mount already covers it — the container
writes `--output /work/.kenn/local/…` and it lands on the host for read-back. No
separate mount is required; `--output` is just one more path arg to prefix-swap.
The only SCIP-specific residue is that it round-trips an output *file* through the
bind mount (owned via Docker Desktop virtualization since `--user` is dropped) —
a manual-smoke verification point, not an architectural one.

### Decision 2b — Reconcile `metadata.project_root` at ingest (the output substitution)

Context fact 2: the indexer emits `metadata.project_root = file:///work` inside
the container, and `canonicalize.rs` drops any record whose
`project_root + relative_path` falls outside the host workspace root (the
`workflow.rs` `all_documents_outside_root` tripwire then hard-errors "empty
index" — so this fails LOUD, not silently, but still fails). Under `Translate`,
ingest MUST map the container root back to the host root before canonicalization:
`mount.to_host(project_root)` — the reverse direction of the same
`ContainerMount` from Decision 2. This is one substitution on the `project_root`
URI (`/work` → `workspace.root()`), NOT a per-file rewrite: each document's
`relative_path` is used unchanged.

*Where it hooks (as built):* `reconcile_container_root` in
`crates/kenn-indexer/src/pipeline/ingest.rs`, called from `absorb_scip_metadata`.
It is **SCIP-only** — the JSONL indexers emit relative paths and carry no
`project_root`, so only the SCIP ingest path needs it. It is **GATED on the
runtime signal**, threaded via a new `ScipDriver::container_mount()` (default
`None`; overridden by the rust/go/python drivers to return their `mount`), NOT by
sniffing the path. This gating is load-bearing: an early unconditional version
regressed the existing `scip_documents_outside_the_root_are_counted` test, which
deliberately uses `/work` as a *sentinel unrelated root* — proving that a real
project_root/workspace mismatch at `/work` must still surface as out-of-root on a
non-Translate run, and only a genuine Translate mount may rebase it.

### Decision 3 — Drop `--user` on Windows

`SamePath` runs `--user {uid}:{gid}` so bind-mount writes are host-owned; on
Windows `current_ids()` already returns `(0,0)`, and Docker Desktop virtualizes
bind-mount ownership (host files are accessible regardless of container uid). So
`Translate` omits `--user` entirely rather than passing a meaningless `0:0`.

*Alternative considered:* keep `--user 0:0`. Rejected — it is noise on Windows and
risks a uid the Docker Desktop VM maps oddly; omission is the documented norm.

### Decision 4 — Windows defaults to docker-on-probe-failure; POSIX stays opt-in

Today `daemon_up = docker && !windows && daemon_available()` forces Windows to
never probe, and the `(true, true, _)` arm returns the "unsupported on Windows"
decline. The new behavior is **platform-differentiated**, matching the practical
reality that native indexer binaries are usually absent on Windows:

- **Probe first, always.** `kenn init` checks each language's indexer on `PATH`
  (the existing probe). A local indexer that is present and runs is used as
  `runtime = "local"` — on Windows too. Docker never overrides a working local
  binary.
- **Windows: docker is the default fallback.** When the local probe FAILS on
  Windows, `init` authors `runtime = "docker"` automatically when the daemon is
  runnable — **without** requiring the explicit `--docker` flag — because Docker
  Desktop is the real indexing route there.
- **Windows without a runnable daemon** falls back to the text degrade, with a
  hint naming Docker Desktop (and the local indexer), rather than erroring on
  every language.
- **POSIX is unchanged.** `--docker` stays opt-in; a failed probe degrades to
  text unless the user asked to containerize. Local toolchains are the norm
  there, so text-degrade is the right default.

Mechanically: drop the `!windows` guard so Windows probes the daemon, and let the
Windows branch treat a failed probe as an implicit containerize (daemon up) or a
text degrade (daemon down). The daemon-absent message names Docker Desktop.

### Decision 5 — Docker Desktop `-v` path form

Emit the host side of the mount in the form Docker Desktop's CLI accepts from
native Windows. Modern Docker Desktop accepts `C:\path` and `C:/path`; the
legacy `//c/path` form is not needed. Settle the exact spelling during apply
against a real Docker Desktop (this is a string-formatting detail, not an
architectural one).

## Risks / Trade-offs

- **Drivers pass more than the root as absolute host paths** → CONFIRMED by the
  apply audit (Decision 2): each driver passes a driver-specific set of absolute
  paths, and SCIP drivers also pass `--output`. *Resolved:* the threaded
  `ContainerMount` prefix-swaps every such arg; derived_root being under the
  workspace bounds `--output` to the `/work` mount. All arg sites are enumerated
  in the Decision 2 table.
- **Docker Desktop bind-mount performance / file-watching over the host↔VM
  boundary** → indexing a large tree may be slow. *Mitigation:* the shared
  dependency-cache named volumes already exist for exactly this reason
  (`docker-indexer-runtime` notes bind-mounted hot caches are slow on Windows);
  the workspace bind-mount is unavoidable but read-mostly.
- **Path-separator drift** (low): the Linux indexer emits `/`-relative paths; a
  future local Windows runtime would emit `\`. *Already handled:* the store's
  `WorkspaceRelativePath` type "always uses `/` regardless of OS", so the docker
  route stores forward-slash paths on Windows by construction. Only a future
  *local* Windows runtime would need normalization — out of scope here.
- **Depends on `windows-support` archiving** (it owns the requirement this
  reverses) → sequence after it. Called out in the proposal.

## Migration Plan

None for data — no reindex, no format change. Rollout is additive: on the next
build, a Windows user with Docker Desktop gets containerized indexing; without
Docker Desktop, the same "requires a runnable daemon" error as any host. Rollback
is reverting the `Translate` variant and restoring the Windows decline.

## Open Questions

- Decision 2: launcher owns the root arg vs. per-driver emission — settle by how
  each driver forms the arg.
- Decision 5: exact Docker Desktop `-v` spelling from native PowerShell.
