## ADDED Requirements

### Requirement: Indexer images are built by Docker from a Dockerfile

Each indexer image SHALL be built by `docker build` from a Dockerfile committed
alongside it. kenn SHALL NOT implement its own OCI layer assembler: a bespoke
builder would be ours to maintain, and a bug in it would break publishing for
every language at once.

#### Scenario: An image builds from its committed Dockerfile

- **WHEN** an indexer image is built
- **THEN** `docker build` produces it from the Dockerfile in that language's
  build context

### Requirement: Everything that can be done in the image build is

Work needed to produce an image SHALL happen inside the build — in a Dockerfile
stage — rather than on the host that invokes it. In particular, binaries baked
into an image SHALL be compiled in a builder stage for the image's own platform,
not cross-compiled on the host and copied in.

The host's job is to invoke the build. Anything more makes the result depend on
what happened to be installed on the machine that ran it.

#### Scenario: The entrypoint is compiled in the build, not on the host

- **WHEN** an image containing the provisioning entrypoint is built
- **THEN** that binary is compiled in a builder stage of the same build
- **AND** the host needs no cross-compilation toolchain for the target platform

#### Scenario: A build needs nothing preinstalled beyond Docker

- **WHEN** an image is built on a machine with only Docker available
- **THEN** the build succeeds

### Requirement: Images contain their payload and no toolchain

Each indexer image SHALL contain its tool binary, the provisioning entrypoint,
the shared libraries they link, a CA certificate bundle, and any auxiliary
executable the indexer spawns at index time (for example `git`, which the
TypeScript and Python indexers both invoke). Images SHALL NOT contain a language
toolchain: the toolchain version belongs to the workspace, and is provisioned at
run time into the shared cache.

The payload is determined by what the indexer actually executes, and SHALL be
established by observing that — not by assuming an indexer is self-contained.
Third-party indexers in particular spawn tools we do not control and cannot patch.

#### Scenario: An image carries no toolchain

- **WHEN** an indexer image is inspected
- **THEN** it contains no language toolchain
- **AND** the toolchain is supplied at run time from the shared cache

#### Scenario: An indexer's spawned tools are present

- **WHEN** an indexer spawns an auxiliary executable during indexing
- **THEN** that executable is present in the image
- **AND** indexing does not degrade or fail for its absence

#### Scenario: Outbound TLS works from the image

- **WHEN** an indexer in a published image fetches dependencies over HTTPS
- **THEN** certificate verification succeeds

### Requirement: A published image is verified by indexing, not by version output

An image SHALL be accepted only after the published artifact is pulled and used
to index a real fixture for its language. A successful `--version` invocation
SHALL NOT be sufficient evidence that an image is correct, because it passes on
an image missing a runtime library or CA bundle that indexing requires.

#### Scenario: Verification indexes a fixture

- **WHEN** a newly published image is verified
- **THEN** the check pulls the published artifact and indexes a fixture for that
  language
- **AND** the check fails if the fixture yields no symbols
