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

### D1 — Sidecar TRIGGERS, `kenn-toolchain` INSTALLS

The first cut of this design had the sidecar download and install the SDK
itself. Implementation and research killed that:

- The C# runtime image has no `curl`/`wget`, so it cannot run
  `dotnet-install.sh`.
- `dotnet` has no first-party install command — `dotnet sdk` exposes only
  `check`, in both SDK 10 and the SDK 11 preview.
- The one community global tool that installs from `global.json`
  (`installsdkglobaltool`) is a 2019, single-author, unmaintained package —
  unacceptable as a dependency in the core indexing path.
- `kenn-toolchain`, our own entrypoint binary, is ALREADY in the image and
  ALREADY downloads, SHA-512-verifies, and atomically installs .NET SDKs.

So the split is: the **sidecar detects and triggers** (it sees the exact project
and its effective `global.json` — the per-project question D2 needs), and
**`kenn-toolchain` installs** (the tested, curl-free, first-party mechanism).
The sidecar shells out to a new `kenn-toolchain provision-sdk <version>`
subcommand on the specific BuildHost SDK-resolution failure, then retries.

This is neither pure-sidecar nor pure-entrypoint: it uses each for what it is
good at, and reuses the download/verify/atomic-cache code that already exists
rather than reimplementing it in a language and image that cannot download.

### D2 — Reactive on BuildHost failure, not a proactive pre-scan

Install only after `OpenProjectAsync` fails with the SDK-resolution error, not
by scanning for `global.json` up front. The reactive path installs exactly the
versions that are actually needed by projects that actually load, and it costs
nothing on the common case where the provisioned SDK already satisfies every
project. A pre-scan would install pins that no reachable project uses.

The failure is identified by the `hostfxr_resolve_sdk2` / "compatible .NET SDK
was not found" signature; anything else is a real load error and is NOT retried.

### D3 — One shared `DOTNET_ROOT`, many SDKs — not a root per version

The current cache gives each toolchain version its own root
(`<arch>/dotnet/<version>/`, a complete install). That does NOT work here: the
Roslyn BuildHost resolves the SDK from a SINGLE `DOTNET_ROOT`, and MSBuildLocator
registers it once per process, so an SDK installed as a separate root is never
found on the retry — verified against the running container.

.NET is built for the opposite layout: ONE root holding `sdk/9.0.316/`,
`sdk/10.0.302/`, … side by side, with `hostfxr` resolving each project's
`global.json` against all of them. So `kenn-toolchain provision-sdk` installs the
pinned SDK INTO the already-exported `DOTNET_ROOT` (`dotnet-install --install-dir
$DOTNET_ROOT`), atomically — stage a temp SDK dir, rename into `sdk/<version>/` —
and the BuildHost finds it on the next evaluation with no re-registration.

Minimal form: `provision-sdk` installs into whatever `DOTNET_ROOT` the entrypoint
already exported (the root-pin SDK's dir), which then holds more than one SDK.
This needs no change to the cache KEY scheme — the extra SDKs live under the
existing root — at the cost that `kenn docker-cache` sees them nested under that
root's version rather than as their own entries. Accurate per-SDK accounting is
a follow-in, not a blocker; the reclaim unit (the whole toolchain volume, or the
root) still works.

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
