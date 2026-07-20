## Why

`foreign-workspace-indexing` shipped `-w` + `kenn init` detection, so kenn can
now set up and index a cloned repo. Dogfooding it surfaced the wall that
motivates this change: **indexing a foreign repo requires that repo's toolchain
on the host.** Two concrete failures from the dogfood run:

1. A foreign Rust repo cloned *inside* the host's cargo workspace fails to index
   — cargo/rust-analyzer walk up, find the ancestor `Cargo.toml`, and abort
   (`error: current package believes it's in a workspace when it's not`). A
   container that mounts only the target repo has no ancestor workspace, so the
   failure disappears.
2. `scip-go` was absent, so a Go repo degraded to text-only (no symbol graph).
   Installing every indexer's toolchain on the agent host is exactly the
   "toolchain sprawl" this change removes.

The design was argued at length during `foreign-workspace-indexing` and
deliberately deferred (see that change's `## Deferred`). This proposal picks it
up. The prior analysis still holds and is the spine here:

- **The path-space silent-skip is a bug today.** Drivers pass absolute *host*
  paths to their tools (`driver/rust.rs:74-77`, and the analogous sites in
  go/python/dotnet/swift/typescript), and `ingest.rs:251` lets the SCIP-declared
  `project_root` override kenn's workspace root. When those two disagree — the
  exact thing a container↔host root mismatch produces — every document fails
  `strip_prefix` and lands in the `OutsideRoot` arm at `ingest.rs:301-307`, which
  **returns `Ok(())` with no counter and no report entry**. Result: empty index,
  exit 0. This must become a counted, reported outcome *before* any container
  work, and it is worth fixing on its own.
- **A pinned image makes the indexer deterministic**, which unlocks a
  content-addressed SCIP cache keyed on `(image digest, config_sig, HEAD)`.
  `compute_staleness_key` (`staleness.rs:117`) already carries `config_sig` +
  HEAD and holds no absolute path; the missing ingredient is a stable *indexer
  identity*, which a host toolchain cannot provide and an image digest can. This
  is the single biggest lever on repeat-clone index time.

Docker is a **fallback, not a default**: a warm local toolchain is faster, so
kenn prefers a local binary when one is present and only reaches for a container
when told to. `kenn init` already reports degraded languages with an install
hint; this change lets it additionally offer the container path.

## What Changes

Delivered in phases; later phases depend on earlier ones.

- **Foundation (bug).** The `OutsideRoot` transform outcome in `ingest.rs`
  becomes counted and surfaced in the `RunReport` instead of a silent skip. Some
  documents dropped but in-root docs survive → a warning and a partial index;
  drops where the run's total in-root count is **zero** → the run **fails with a
  non-zero exit**, instead of today's silent empty index at exit 0. An
  honestly-empty run (no drops at all) stays a success.
- **Config surface.** A per-language `runtime` (`"local"` default, or
  `"docker"`) and `image` (an OCI reference) on each of the six code-driver
  language configs. `command` keeps its meaning as the **in-container** argv.
  `indexing_signature` already hashes the whole `[language.*]` tree, so these
  fields fold into the reindex key for free.
- **Docker runtime.** `configure_runner` (`workflow.rs:220-278`) — the single
  config→driver funnel — synthesizes a `docker run` wrapper around the
  configured launcher when `runtime = "docker"`. On POSIX hosts the workspace is
  bind-mounted **at its own absolute path** (`-v /host/ws:/host/ws`), so every
  absolute-path argument the drivers already emit resolves unchanged and the
  SCIP `project_root` matches kenn's root — no per-driver path rewriting.
- **Dependency caches.** A container has the toolchain but not the foreign repo's
  third-party dependencies, which the indexer resolves at run time (the 3+ minutes
  the dogfood's isolated rust-analyzer spent). `configure_runner` bind-mounts a
  kenn-managed host cache dir and points each ecosystem's cache env var
  (`CARGO_HOME`, `GOMODCACHE`, `NUGET_PACKAGES`, …) at it, so the first index
  fetches and later indexes — and other repos in the same ecosystem — run warm.
- **Published images + CI (kenn-owned).** kenn maintains a Dockerfile per indexer
  and a CI workflow that builds and pushes `ghcr.io/kenn/<indexer>` images, pinned
  by digest; `kenn init --docker` and the defaults reference those digests. This
  is greenfield: the repo has no `.github/`, no Dockerfiles, and no
  container/publish recipes today — distribution is host-native binaries via
  `just install`. kenn accepts ownership of the six images' maintenance, size, and
  security scanning.
- **`kenn init --docker`.** When a language's local toolchain is absent (the
  probe that already drives the degrade report) and `docker` is runnable, init
  writes `runtime = "docker"` + that language's default published (digest-pinned)
  `image` instead of degrading it to text. Depends on the published images above.
- **Content-addressed SCIP cache.** A cache keyed on
  `(resolved image digest, config_sig, HEAD)` that lets a repeat clone reuse a
  prior identical SCIP output instead of re-running the container. Sound only
  under a pinned image; host runtimes do not populate it.

## Capabilities

### Added Capabilities

- **docker-indexer-runtime** — run a language's indexer inside a pinned
  container instead of a host binary, with a reported (not silent) out-of-root
  outcome, an image-digest-keyed output cache, and `kenn init --docker` opt-in.

## Impact

- **New config fields** on six `[language.*]` blocks (`kenn-config`). Additive
  and defaulted (`runtime = "local"`); existing configs are unaffected.
- **`ingest.rs` behavior change**: out-of-root documents are now reported. A run
  that today silently produced an empty index at exit 0 will now **exit non-zero**
  — this is the point, but it changes the exit contract for that (already-broken)
  case. A partial drop stays exit 0 with a warning.
- **New build/release surface**: Dockerfiles + a GitHub Actions workflow +
  `just` recipes for images. First CI in the repo.
- **Runtime dependency**: the docker path requires a running Docker daemon on the
  host; absence is a reported, actionable error, never a silent fallback.
- **Container file ownership**: containerized runs execute as the invoking uid
  with a writable `HOME` and kenn-owned host cache bind mounts, so files written
  into the workspace and caches are user-owned, not root.
- **Shared on-disk caches**: the docker path adds two kenn-owned host caches (the
  SCIP output cache and the package cache), each with its own bounded LRU keyed on
  a directory disjoint from the per-workspace vector cache (tracked as a risk).

## Deferred

- **Windows path translation.** POSIX (macOS/Linux) ships here via the same-path
  mount. Windows host paths are not valid Linux container paths, so it needs a
  `/work` mount plus bidirectional path translation of driver arguments and the
  SCIP `project_root` — a **separate follow-on change**. The `MountStrategy` seam
  (`SamePath` now, `Translate` later) is built here so it drops in cleanly.
- **Podman / other OCI runtimes.** `runtime = "docker"` first; the wrapper is
  shaped so a `"podman"` value is a small follow-on, not a redesign.
- **Rootless/remote daemons and non-default socket paths** beyond what `docker`
  on `PATH` resolves by default.
- **Multi-arch image fan-out** past what the host architecture needs to run.
