## MODIFIED Requirements

### Requirement: kenn synthesizes the container invocation

When a language's `runtime = "docker"`, kenn SHALL run the configured `command`
inside the `image` via `docker run`. On POSIX hosts kenn SHALL bind-mount the
workspace at its own absolute path so the absolute paths the drivers pass resolve
inside the container and the indexer's `project_root` reconciles with kenn's
workspace root without translation. On Windows kenn SHALL bind-mount the
workspace at the fixed container path `/work` with `/work` as the working
directory, SHALL translate the workspace-root argument the driver passes from the
host path to `/work`, AND SHALL reconcile the indexer's reported `project_root`
(`/work`) back to the host workspace root when ingesting its records — otherwise
canonicalization drops every record as resolving outside the workspace root and
the index is silently empty. No per-file path rewrite is required, because each
document's path is workspace-relative. kenn SHALL run the container as the
invoking user's uid/gid with a writable `HOME` on POSIX hosts, so files it writes
into the workspace (the SCIP output and `.kenn/`) are owned by that user rather
than root; on Windows, where bind-mount ownership is virtualized by the Docker
host, kenn SHALL NOT pass `--user`.

#### Scenario: POSIX same-path mount needs no translation

- **WHEN** a POSIX host runs a docker-runtime language
- **THEN** kenn mounts the workspace root at the identical absolute path with
  that path as the working directory
- **AND** the driver's absolute-path arguments and the emitted SCIP output land
  on the host unchanged

#### Scenario: Windows translated mount rewrites the root arg and reconciles project_root

- **WHEN** a Windows host runs a docker-runtime language
- **THEN** kenn mounts the workspace at `/work` with `/work` as the working
  directory and passes `/work` as the driver's workspace-root argument
- **AND** kenn reconciles the indexer's reported `project_root` (`/work`) to the
  host workspace root at ingest, so the workspace-relative records are retained
  rather than dropped as outside the workspace root
- **AND** no per-file path rewrite is performed

#### Scenario: Container-written files are owned by the invoking user

- **WHEN** a docker-runtime index writes SCIP output and `.kenn/` scaffolding into
  the mounted workspace
- **THEN** those files are owned by the invoking user, so a subsequent `kenn` run
  can overwrite them rather than hitting root-owned files

#### Scenario: A running Docker daemon is required and its absence is reported

- **WHEN** any language is configured `runtime = "docker"` and `docker` is not
  runnable on the host
- **THEN** the run fails with an actionable error naming the missing daemon
- **AND** kenn does NOT silently fall back to a local toolchain

## ADDED Requirements

### Requirement: kenn init on Windows probes local indexers first, then defaults to docker

On Windows, `kenn init` SHALL probe each detected language's indexer on `PATH`
and, when that probe succeeds, author `runtime = "local"` — a working local
indexer is used on Windows as on any host, and docker SHALL NOT override it.

When the local probe FAILS on Windows and a default image exists, `kenn init`
SHALL author `runtime = "docker"` when the Docker daemon is runnable, **without
requiring the `--docker` flag** — Docker Desktop is the default indexing route on
Windows. When the daemon is NOT runnable, `init` SHALL degrade the language to the
text fallback and name Docker Desktop (and the local indexer) in the hint, rather
than erroring. kenn SHALL NOT refuse the docker runtime on Windows solely because
the host is Windows.

On POSIX hosts this default is unchanged: a failed probe degrades to text unless
`--docker` was requested.

This supersedes the prior requirement that the docker runtime is unsupported on
Windows: Windows path translation, its stated blocker, is now implemented.

#### Scenario: A working local indexer on Windows is used, not containerized

- **WHEN** `kenn init` runs on Windows and a language's indexer is on `PATH` and
  passes its probe
- **THEN** the authored config sets that language to `runtime = "local"`
- **AND** it is not containerized

#### Scenario: A missing local indexer on Windows defaults to docker

- **WHEN** `kenn init` runs on Windows where a language's local probe fails, the
  daemon is runnable, and a default image exists
- **THEN** the authored config sets that language to `runtime = "docker"` with the
  default image, without the `--docker` flag being passed
- **AND** the report lists the language as containerized, not `degraded → text`

#### Scenario: Windows without Docker Desktop degrades with a hint

- **WHEN** `kenn init` runs on Windows, a language's local probe fails, and the
  Docker daemon is not runnable
- **THEN** the language degrades to the text fallback
- **AND** the hint names Docker Desktop and the local indexer
- **AND** init does not claim docker is categorically unsupported on Windows
