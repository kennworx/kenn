# docker-indexer-runtime Specification

## Purpose
TBD - created by archiving change docker-indexer-runtime. Update Purpose after archive.
## Requirements
### Requirement: Out-of-root documents are counted and reported

The ingest pipeline SHALL count and report — not silently skip — any document
whose canonicalized path falls outside kenn's workspace root
(`CanonicalizeError::OutsideRoot`). When such drops occur but the run still has
in-root documents, kenn SHALL warn and produce a partial index. When such drops
occur **and** the run's total in-root document count is zero, kenn SHALL fail with
a non-zero exit rather than report a successful, empty index. A run with no
out-of-root drops (including an empty repository or one whose files are all
excluded) SHALL NOT fail on this account. Intentional exclusions
(`CanonicalizeError::Excluded`) remain silent.

#### Scenario: An all-outside-root run fails instead of emitting an empty index

- **WHEN** an indexer produces documents but reports a `project_root` (e.g.
  `/work`) that is not a prefix of kenn's root, so every document fails
  `strip_prefix` and no in-root document survives
- **THEN** the run fails with a non-zero exit naming the mismatch
- **AND** no snapshot is reported as a clean success with an empty index

#### Scenario: A partial out-of-root drop warns and keeps a partial index

- **WHEN** some but not all documents fall outside the root
- **THEN** the `kenn index` summary prints a warning with the drop count
- **AND** the run exits 0 with a partial snapshot

#### Scenario: An empty run is not treated as a failure

- **WHEN** a run has no out-of-root drops (an empty repository, or one whose files
  are all excluded)
- **THEN** the run does not fail on the out-of-root account and exits 0

#### Scenario: Intentional exclusions stay silent

- **WHEN** a document matches a configured exclude and is dropped as `Excluded`
- **THEN** no out-of-root warning is emitted for it

### Requirement: A language may run its indexer in a container

Each code-driver language (`csharp, rust, typescript, python, go, swift`) SHALL
accept a `runtime` (`"local"` default, or `"docker"`) and an `image` (an OCI
reference) in its `[language.*]` config. `command` retains its meaning as the argv
executed inside the runtime. A `runtime` or `image` change SHALL invalidate the
reindex staleness key. A `"docker"` runtime without an `image` SHALL be rejected
at config validation.

#### Scenario: Docker runtime is configured per language

- **WHEN** a user sets `[language.rust] runtime = "docker"` with an `image` and a
  `command`
- **THEN** the config loads with rust's runtime as docker
- **AND** flipping `runtime` back to `"local"` changes `indexing_signature`

#### Scenario: Docker runtime without an image is rejected

- **WHEN** a config sets `runtime = "docker"` for a language but omits `image`
- **THEN** config validation fails with an actionable error

### Requirement: kenn synthesizes the container invocation

When a language's `runtime = "docker"`, kenn SHALL run the configured `command`
inside the `image` via `docker run`, bind-mounting the workspace at its own
absolute path (POSIX hosts) so the absolute paths the drivers pass resolve inside
the container and the indexer's `project_root` reconciles with kenn's workspace
root without translation. kenn SHALL run the container as the invoking user's
uid/gid with a writable `HOME`, so files it writes into the workspace (the SCIP
output and `.kenn/`) are owned by that user rather than root. Windows path
translation is a separate follow-on change.

#### Scenario: POSIX same-path mount needs no translation

- **WHEN** a POSIX host runs a docker-runtime language
- **THEN** kenn mounts the workspace root at the identical absolute path with
  that path as the working directory
- **AND** the driver's absolute-path arguments and the emitted SCIP output land
  on the host unchanged

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

### Requirement: Container indexers reuse persistent dependency caches

When running an indexer in a container, kenn SHALL mount a kenn-managed **named
Docker volume** as the dependency-source cache and point each language ecosystem's
package-cache location at it via an explicit env var (`CARGO_HOME`, `GOMODCACHE`,
`NUGET_PACKAGES`, …), so third-party dependencies are fetched once and reused across
runs. A named volume (not a host bind mount) is used because a bind-mounted hot
cache is severely slow across the host↔VM boundary on macOS/Windows.

By default this cache SHALL be **per repository**: bound to the repository's main
worktree directory (resolved via git; the workspace root itself when not a worktree)
and shared by all of that repository's linked worktrees — so a dependency is fetched
once per repo and reused by every worktree, and the cache stays reclaimable when the
repository is deleted (see the reclaim requirement). Configuration MAY instead select
a single shared cross-repository volume (fetch once across all repos); that volume is
bound to no directory and so is never reclaimed automatically. Build-artifact caches
are never shared this way — they stay per-worktree, because the toolchain locks its
build directory.

#### Scenario: A repository's worktrees share one dependency-warm cache

- **WHEN** a repository is indexed, then a linked worktree of the same repository is
  indexed with the container network disabled
- **THEN** the worktree's index resolves its dependencies from the repository's cache
  and succeeds offline

#### Scenario: Different repositories do not share a dependency cache by default

- **WHEN** two different repositories are indexed with no shared cache configured
- **THEN** each gets its own `kenn-deps-<hash>` volume bound to its own main worktree
- **AND** neither reuses the other's downloaded dependencies

#### Scenario: A configured shared cache spans repositories

- **WHEN** the shared cross-repository dependency volume is configured and two
  different repositories are indexed
- **THEN** the second repository reuses the dependencies the first one fetched

### Requirement: kenn init can opt a degraded language into a container

`kenn init --docker` SHALL, for a language whose local toolchain probe fails and
whose default image is known, author `runtime = "docker"` with that image instead
of degrading the language to the text fallback, and report it as containerized.
Without a runnable `docker`, `--docker` SHALL report an error and detection SHALL
fall back to the existing degrade report. Behavior is a non-interactive report,
never a prompt.

#### Scenario: A missing toolchain is containerized instead of degraded

- **WHEN** `kenn init --docker` runs where a language's toolchain is absent but
  `docker` is runnable and a default image exists
- **THEN** the authored config sets that language to `runtime = "docker"` with the
  default image
- **AND** the report lists the language as containerized, not `degraded → text`

#### Scenario: init --docker with no docker falls back to degrade

- **WHEN** `kenn init --docker` runs and `docker` is not runnable
- **THEN** init reports an actionable error
- **AND** detection still produces the normal degrade report

### Requirement: Indexer images are published and pinned by digest

kenn SHALL provide a container image per indexer, published to a registry by CI,
and its default and `kenn init --docker` image references SHALL be pinned by
digest rather than by mutable tag.

An image SHALL NOT carry a language toolchain: the toolchain version is a
property of the workspace being indexed, not of the image, and is supplied at run
time from the shared toolchain cache. An image MAY carry the auxiliary tools its
indexer spawns — `git` for the TypeScript and Python indexers, for example — and
those are part of its payload, not a toolchain.

#### Scenario: Default images resolve to a digest

- **WHEN** `kenn init --docker` writes a default image for a language
- **THEN** the written `image` is a digest-pinned reference

#### Scenario: An image carries no toolchain

- **WHEN** a published indexer image is inspected
- **THEN** no language toolchain is present in it

#### Scenario: One image serves every declared toolchain version

- **WHEN** two workspaces declaring different toolchain versions for the same
  language are indexed
- **THEN** both use the same digest-pinned image
- **AND** each is indexed with its own declared toolchain version

### Requirement: kenn can reclaim its Docker cache volumes

kenn SHALL provide a `kenn docker-cache` command to list and remove the named Docker
volumes its container runtime creates but Docker never garbage-collects. They are of
three kinds. Two are **bound to a directory** and named by a one-way hash of it: a
per-worktree **build** volume (`kenn-build-<hash>`, bound to the worktree, created
only under `persist_build_cache`) and a per-repository **dependency** volume
(`kenn-deps-<hash>`, bound to the repository's main worktree — unless a shared
cross-repository volume is configured, which is bound to nothing). The third is the
machine-wide **toolchain** volume (`kenn-toolchains`), which holds provisioned
language toolchains, is shared by every workspace on the machine, and is therefore
bound to no directory. Each hash SHALL be produced by a single function shared by the
indexer that creates the volume and the command that removes it, so the two never
disagree.

Because the name is a one-way hash, kenn SHALL label the volumes it creates so the
command can find and reason about them with no reversible name, no external registry,
and no need to read any workspace config. Every kenn-created volume SHALL carry a
`kenn.managed` Docker label — the **enumeration** key, by which the command discovers
all of kenn's volumes (anything without it is not kenn's concern). Each **bound**
volume (a build volume, and a per-repository dependency volume) SHALL additionally
carry a `kenn.workspace` label holding its bound directory's absolute path — the
**orphan-binding** key, by which cleanup tells whether that directory still exists. A
configured shared cross-repository dependency volume, and the toolchain volume, are
`kenn.managed` but bound to no directory, so they carry no `kenn.workspace` label and
are therefore never orphans.

`kenn docker-cache ls` SHALL list every `kenn.managed` volume — each with its kind
(read from the `kenn-build-`/`kenn-deps-`/`kenn-toolchains` name), its bound
`kenn.workspace` path (or `shared`), whether that path still exists on disk, and
whether the volume is attached to a container (in-use) — with a `--json` form for
tooling. For the toolchain volume, `ls` SHALL additionally report the provisioned
toolchains it holds, by language and resolved version, with their sizes, so a user
can see what is consuming the space before deciding what to reclaim.

`kenn docker-cache clean` SHALL remove volumes, scoped by exactly one (mutually
exclusive) mode: no flag removes only the **current worktree's build** volume;
`--orphans` removes every `kenn.managed` volume that carries a `kenn.workspace` label
whose directory **no longer exists** — so dropping a worktree reclaims that worktree's
build volume while the repository's dependency volume (bound to the still-present main
worktree) survives, and deleting the whole repository reclaims both its build and its
dependency volumes, besides self-healing any volume leaked by an earlier crash;
`--all` removes every `kenn.managed` volume regardless of binding (including a
configured shared volume and the toolchain volume); `--workspace <path>` removes the
build volume for an existing worktree at `<path>` — targeting a repository's dependency
volume directly is out of scope (it is reclaimed by `--orphans` on repository deletion,
or removed with `docker volume rm`). Because the toolchain volume is the largest and is
expensive to repopulate, it SHALL also be targetable on its own: `--toolchains`
removes it entirely, and `--toolchain <language>[@<version>]` removes only the named
provisioned toolchains, leaving the volume and its other occupants intact. Because
`--orphans` keys on on-disk existence, a directory whose
drive or network mount is currently detached is treated as gone and its volume
reclaimed — accepted, since caches rebuild; likewise, deleting a repository's main
worktree while linked worktrees remain (which already breaks git, since the shared
`.git` lives there) reaps the deps cache, harmlessly.

Removal outcomes SHALL be reported and mapped to exit status as follows: an absent
target volume is reported as nothing-to-remove with **exit 0** (so teardown never
fails on a workspace that never persisted a build cache); a volume attached to a
running container is reported as skipped-in-use with **exit 0**; a genuine Docker
failure (permission denied, daemon error mid-sweep) is reported and SHALL **exit
non-zero**, even when other volumes in a sweep were removed. When `docker` is not
runnable, `clean` SHALL report docker unavailable and **exit 0** (teardown-safe),
removing nothing, whereas `ls` SHALL fail with an actionable error.

#### Scenario: Dropping a worktree reaps its build volume but keeps the repo's deps

- **WHEN** a linked worktree indexed under `persist_build_cache` is deleted and
  `kenn docker-cache clean --orphans` is run while the repository's main worktree
  still exists
- **THEN** the worktree's `kenn-build-<hash>` volume is removed
- **AND** the repository's `kenn-deps-<hash>` volume, bound to the live main worktree,
  is left intact

#### Scenario: Deleting the whole repository reaps its build and deps volumes

- **WHEN** an entire repository directory (main worktree and all linked worktrees) is
  deleted and `kenn docker-cache clean --orphans` is run
- **THEN** both its `kenn-build-<hash>` volumes and its `kenn-deps-<hash>` volume are
  removed, because none of their bound directories exist
- **AND** a configured shared cross-repository dependency volume, bound to nothing, is
  never treated as an orphan

#### Scenario: The toolchain volume survives orphan sweeps

- **WHEN** `kenn docker-cache clean --orphans` runs after a repository is deleted
- **THEN** the toolchain volume is left intact, because it is bound to no directory
- **AND** a subsequent index of another workspace still finds its toolchains
  provisioned

#### Scenario: Toolchains are listed and reclaimed individually

- **WHEN** `kenn docker-cache ls` runs with two provisioned toolchains
- **THEN** the toolchain volume is listed with each toolchain's language, resolved
  version, and size
- **AND WHEN** `kenn docker-cache clean --toolchain <language>@<version>` is run
- **THEN** only that toolchain is removed and the others remain provisioned

#### Scenario: ls shows kind, binding, existence, and in-use

- **WHEN** `kenn docker-cache ls` runs with a build volume for a live worktree
  (attached to a running container) and a deps volume for a deleted repository
- **THEN** each is listed with its kind, its bound path (or `shared`), its
  exists/missing state, and its in-use state
- **AND** `--json` emits the same information as machine-readable output

#### Scenario: Bare clean targets only the current worktree

- **WHEN** `kenn docker-cache clean` runs with no flags inside a worktree
- **THEN** only that worktree's `kenn-build-<hash>` volume is removed
- **AND** other worktrees' build volumes, the repository's dependency volume, and the
  toolchain volume are left intact

#### Scenario: --all sweeps every kenn volume

- **WHEN** `kenn docker-cache clean --all` runs
- **THEN** every kenn volume — build, dependency, and toolchain, bound or shared — is
  removed
- **AND** a volume attached to a running container is skipped, not removed

#### Scenario: Cleaning is idempotent, in-use-safe, and error-honest

- **WHEN** a clean mode targets a volume that does not exist
- **THEN** it reports nothing to remove and exits 0
- **AND WHEN** it targets a volume attached to a running container
- **THEN** that volume is reported as skipped (in use) and it still exits 0
- **AND WHEN** a `docker volume rm` fails for an unexpected reason during a sweep
- **THEN** the command reports the failure and exits non-zero

#### Scenario: docker unavailable is teardown-safe for clean but an error for ls

- **WHEN** `docker` is not runnable and `kenn docker-cache clean` is invoked
- **THEN** it reports docker unavailable and exits 0 without removing anything
- **AND WHEN** `kenn docker-cache ls` is invoked while `docker` is not runnable
- **THEN** it fails with an actionable error

### Requirement: Container indexers mount the shared toolchain cache

When running an indexer in a container, kenn SHALL mount the kenn-managed
toolchain cache and point the language's toolchain root at it, so a provisioned
toolchain is shared across every workspace on the machine. This cache is distinct
from the per-repository dependency cache: dependencies belong to a repository,
toolchains do not.

#### Scenario: A toolchain provisioned for one repository serves another

- **WHEN** two different repositories resolving to the same toolchain version are
  indexed in sequence
- **THEN** the second run reuses the provisioned toolchain without downloading it

#### Scenario: Toolchain and dependency caches are separate

- **WHEN** a repository's dependency cache is reclaimed
- **THEN** the shared toolchain cache is unaffected
- **AND** a subsequent index still finds its toolchain provisioned

### Requirement: Provisioning happens inside the indexer container

Every indexer image SHALL run a kenn-authored entrypoint that resolves the
workspace's pinned toolchain, provisions it into the mounted cache when absent,
and then execs the indexer. kenn SHALL NOT orchestrate toolchain downloads from
the host; its only use of `docker` beyond running indexers is building images.

The entrypoint SHALL be present in every image, including those whose indexer is
a third-party binary with no kenn code in it — that is the only uniform place
provisioning can happen for those languages.

#### Scenario: A third-party indexer still gets its toolchain

- **WHEN** a language whose indexer is a third-party binary is indexed and its
  pinned toolchain is absent from the cache
- **THEN** the entrypoint provisions it before the indexer runs
- **AND** indexing completes using that toolchain

#### Scenario: The host does not download toolchains

- **WHEN** a containerized language is indexed
- **THEN** kenn performs no toolchain download on the host

### Requirement: A changed toolchain pin forces a reindex

An index SHALL record the resolved toolchain version it was produced with, and
SHALL report it in the index run summary. A workspace's pin files are tracked
content, so editing one already changes the staleness key and triggers a reindex;
this requirement adds no separate signature mechanism, and the recorded version
exists so a change is visible and attributable rather than silent.

#### Scenario: Editing the pin file makes the workspace stale

- **WHEN** a workspace's toolchain pin file is edited to name a different version
- **THEN** the workspace is reported stale
- **AND** the next index runs against the newly resolved toolchain

#### Scenario: The run summary names the toolchain it used

- **WHEN** an index run completes
- **THEN** the summary reports the resolved toolchain version used for each
  containerized language

