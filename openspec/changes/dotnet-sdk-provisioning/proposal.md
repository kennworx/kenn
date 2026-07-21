## Why

A C# repository that pins its SDK in a **subdirectory** cannot be indexed
through the docker runtime, and the failure is silent.

Newtonsoft.Json is the case. Its `Src/global.json` pins SDK `9.0.300`
(rollForward `latestFeature`). kenn provisions toolchains from the **root** pin
file only — there is no `global.json` at the repo root, so it provisions the
default (latest) SDK, 10.x. The Roslyn BuildHost then evaluates each project,
honors the nested `Src/global.json`, and fails:

```
Error while calling hostfxr function hostfxr_resolve_sdk2:
A compatible .NET SDK was not found.
```

Every project loads as zero documents. The index reports 0 C# files at exit 0 —
the exact "succeeded but indexed nothing" shape this project keeps fighting.

The current design makes this unreachable: provisioning is entrypoint-side and
driven by the root pin, and an unsatisfiable pin is deliberately fatal-with-a-
diagnostic rather than falling back. That is correct for a root pin the
entrypoint can see. It has no answer for a pin the entrypoint never looked at.

## What Changes

**kenn-dotnet gains the ability to install the exact SDK a project pins, on
demand, behind an opt-in flag** — the mechanism the user proposed.

When the BuildHost reports "no compatible SDK", the sidecar reads the effective
`global.json` for that project, installs the pinned SDK version into the shared
toolchain cache via the official `dotnet-install` script, adds it to the SDK
search path, and retries the load. Off by default; enabled with
`--provision-sdk` (and the matching `kenn.toml` key).

Why the sidecar rather than the entrypoint: a repo can carry several nested
`global.json` files pinning different SDKs, and the BuildHost selects per
project. Installing "the pin" is a per-project question the sidecar answers as
it loads; the entrypoint, provisioning once up front from a single root pin,
structurally cannot.

**Opt-in, not default**, because it makes the indexer reach the network at
index time and download an SDK (~200MB), and because it softens the
deliberately-fatal-pin contract. A default-off flag keeps the strict behavior
the default and makes the network-touching path a choice.

## Capabilities

### New Capabilities
- `dotnet-sdk-provisioning`: how kenn-dotnet installs a pinned SDK it does not
  have, when it does so, where the SDK lands, and how the strict
  fail-on-unsatisfiable-pin behavior is preserved when the flag is off.

### Modified Capabilities
- `kenn-dotnet-runtime`: the sidecar currently treats an unresolvable SDK pin as
  a terminal, diagnostic-only failure. Under the new flag it may instead
  install and retry.

## Impact

**Code** — `indexers/kenn-dotnet` (the install-and-retry path, the flag, the
`global.json` version read), `crates/kenn-config` (the `kenn.toml` key), and the
docker launcher, which must pass the shared toolchain cache in a form the
sidecar can write a new SDK into.

**Network at index time** — a new behavior for the indexer, and the reason it is
opt-in. Must be bounded (timeout) and its failure must be a named diagnostic,
not a hang.

**The fatal-pin directive** — this revises "an unresolvable pin must be FATAL;
never fall back." The revision is narrow: with the flag OFF the directive holds
unchanged; with it ON, "resolve" is allowed to mean "install the pinned version"
rather than "use a present one" — it never falls back to a DIFFERENT version,
which is what the directive actually guards against.

**Not this change** — provisioning nested pins from the entrypoint, and the same
capability for other languages. Rust/Go/Python run third-party indexers with no
sidecar of ours to host this; if nested-pin support is wanted there, it belongs
in the entrypoint and is a separate design.
