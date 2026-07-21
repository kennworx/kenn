## MODIFIED Requirements

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

## ADDED Requirements

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
