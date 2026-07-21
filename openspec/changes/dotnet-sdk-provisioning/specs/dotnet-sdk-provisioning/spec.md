## ADDED Requirements

### Requirement: kenn-dotnet can install a pinned SDK on demand

When enabled, kenn-dotnet SHALL install the exact SDK version a project's
effective `global.json` pins, if loading that project fails because no
compatible SDK is present, and then retry the load.

The capability SHALL be OFF by default, enabled by a `--provision-sdk` flag and
the matching `kenn.toml` key. With it off, an unsatisfiable pin remains a
terminal, named failure — the strict behavior is the default.

This exists for the nested-`global.json` case: kenn provisions from the root pin
file, so a repo pinning its SDK in a subdirectory (Newtonsoft.Json's
`Src/global.json`) gets a different SDK than its projects require, and every
project loads as zero documents.

#### Scenario: a nested pin is satisfied by installing

- **WHEN** provisioning is enabled and a project fails to load because its
  `global.json`-pinned SDK is not present
- **THEN** kenn-dotnet installs that pinned version
- **AND** retries the load
- **AND** the project's symbols appear in the index

#### Scenario: provisioning is off by default

- **WHEN** the flag is not set and a project's pinned SDK is absent
- **THEN** kenn-dotnet does not install anything
- **AND** reports the unsatisfied pin as a named failure, as it does today

### Requirement: only the SDK-resolution failure triggers an install

kenn-dotnet SHALL install-and-retry only for the specific failure of a project's
SDK pin being unsatisfiable (the `hostfxr_resolve_sdk2` / "compatible .NET SDK
was not found" signature). Any other load failure SHALL NOT trigger an install.

Retrying an unrelated failure by installing an SDK wastes a download and hides
the real error behind a second, misdirected one.

#### Scenario: an unrelated load error is not retried with an install

- **WHEN** a project fails to load for a reason other than an unsatisfiable SDK
  pin
- **THEN** no SDK is installed
- **AND** the original error is reported

### Requirement: installs never use a different version than pinned

An install SHALL provide the version the pin names (subject to the pin's own
`rollForward`). kenn-dotnet SHALL NOT satisfy a pin with a different version.

This preserves what the fatal-pin rule protects: a project is never indexed
against an SDK its `global.json` would reject, because that silently produces a
wrong or empty result.

#### Scenario: the installed version matches the pin

- **WHEN** a `global.json` pins `9.0.300`
- **THEN** the installed SDK satisfies `9.0.300` under that file's `rollForward`
- **AND** no other major/minor is substituted

### Requirement: installs are atomic, shared, and reclaimable

An installed SDK SHALL land in the shared toolchain cache under the same
`<arch>/dotnet/<version>` layout the entrypoint uses, written via a
stage-then-rename so a partial download is never seen as complete.

A version once installed SHALL be reused by later runs and by other projects
needing it, and SHALL be visible to `kenn docker-cache` like any other
provisioned toolchain.

#### Scenario: a second run reuses the installed SDK

- **WHEN** a run installs a pinned SDK, and the same workspace is indexed again
- **THEN** the second run does not re-download it

#### Scenario: an interrupted install leaves nothing usable

- **WHEN** an install is interrupted partway
- **THEN** no partial directory is present that a later run would treat as a
  complete SDK

### Requirement: install failures are bounded and named

An install that cannot complete SHALL fail with a diagnostic that names the pin
and its source file, within a bounded time — whether the cause is a network
failure, a timeout, or a pin naming a version that does not exist. It SHALL NOT
hang, and SHALL NOT retry the same version indefinitely.

#### Scenario: a pin names a nonexistent SDK

- **WHEN** provisioning is enabled and a `global.json` pins a version that
  cannot be installed
- **THEN** the failure names the pinned version and the `global.json` path
- **AND** the run does not hang
