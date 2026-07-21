## Context

Provisioning today is entrypoint-side. `kenn-toolchain` reads the workspace's
root pin file, installs that toolchain into the shared cache volume, puts it on
PATH, and execs the indexer. For an unsatisfiable or absent pin it either
provisions the default or fails with a named diagnostic — it never silently uses
a different version.

That model has one blind spot: a `global.json` in a subdirectory. MSBuild and
the Roslyn BuildHost resolve the SDK by walking UP from each project, so a
`Src/global.json` governs `Src/**` regardless of what the entrypoint provisioned
from the root. When the pinned SDK is absent, `hostfxr_resolve_sdk2` fails and
the project loads as zero documents.

kenn-dotnet is the one indexer that can act on this itself: it is our binary, it
is the process that hits the BuildHost failure, and it knows the project whose
pin was unsatisfied.

## Goals / Non-Goals

**Goals:**
- A C# repo with a nested `global.json` indexes through the docker runtime.
- The install is opt-in and its cost (network, disk, time) is visible.
- The strict fail-on-unsatisfiable-pin behavior is unchanged when off.
- Installed SDKs are shared and reused, not re-downloaded per run.

**Non-Goals:**
- Entrypoint-side nested-pin discovery (a valid alternative — see Decisions).
- The same for rust/go/python.
- Installing runtimes or workloads beyond the SDK the pin names.

## Decisions

### D1 — Sidecar-side install, not entrypoint-side

Two designs solve the nested pin:

- **A (this change): the sidecar installs on BuildHost failure.** It sees the
  exact project and its effective `global.json`, so it installs precisely what
  was needed, and it handles a repo with several different nested pins
  naturally — each miss installs its own version.
- **B: the entrypoint discovers nested `global.json` files and pre-provisions
  them.** Keeps provisioning in one place, consistent with the current model,
  but must find and reconcile every nested pin up front, provision all of them,
  and still leave the BuildHost to pick — more work done eagerly, some of it
  wasted on pins no loaded project actually uses.

A is chosen because installing is inherently a per-project, on-demand question
and the sidecar is where that question is asked. B remains the right home if
this is ever wanted for the third-party languages, which have no sidecar.

### D2 — Reactive on BuildHost failure, not a proactive pre-scan

Install only after `OpenProjectAsync` fails with the SDK-resolution error, not
by scanning for `global.json` up front. The reactive path installs exactly the
versions that are actually needed by projects that actually load, and it costs
nothing on the common case where the provisioned SDK already satisfies every
project. A pre-scan would install pins that no reachable project uses.

The failure is identified by the `hostfxr_resolve_sdk2` / "compatible .NET SDK
was not found" signature; anything else is a real load error and is NOT retried.

### D3 — Install into the shared toolchain cache, atomically

The SDK lands in the same `/kenn-toolchains/<arch>/dotnet/<version>` layout the
entrypoint uses, via the official `dotnet-install` script, staged-and-renamed so
a partial download is never seen as a complete SDK — the same atomic-install
contract the entrypoint's cache already relies on. A second run, or another
project needing the same version, reuses it.

The docker launcher must therefore mount the toolchain cache writable by the
sidecar's uid, which it already does for the entrypoint.

### D4 — Off by default; the strict contract is the default

`--provision-sdk` (and `[language.csharp] provision_sdk` in kenn.toml) default
off. With it off, an unsatisfiable pin stays exactly as fatal and as
diagnostic-rich as today. With it on, "resolve" may mean "install the pinned
version" — it still never uses a DIFFERENT version, which is what the fatal-pin
directive actually protects against.

### D5 — Retry once per version, then give up loudly

After installing a version, retry the failed load once. A second failure for the
same version is a real problem (corrupt install, a pin naming a version that
does not exist) and is reported as a named diagnostic, not retried again. An
install that itself fails (network, unknown version) is likewise a named
failure, never a hang.

## Risks / Trade-offs

**Network at index time.** New for the indexer, and the reason for the flag. A
bounded timeout and a named failure are required; a silent retry loop or a hang
would be worse than the current honest "no compatible SDK".

**A pin naming a nonexistent SDK.** `dotnet-install` fails; the change must
surface that as "global.json pins X, which could not be installed" — naming the
pin file — rather than a raw script error.

**Disk growth.** Each distinct pinned SDK is ~200MB in the shared cache.
Acceptable (the cache is already the largest thing kenn stores and is
reclaimable via `kenn docker-cache`), but the installed SDKs must be visible to
that tooling like any other provisioned toolchain.

**Verifying it.** The honest test is a repo with a nested `global.json` pinning
an SDK the container does NOT have — Newtonsoft.Json is exactly that. A test that
stages an already-present SDK proves only the flag plumbing, not the install.
