# Design

## Context

Grounded in the current code (anchors verified 2026-07-11):

- **Config**: no shared per-language base struct and no macro — each of the six
  code-driver languages (`csharp, rust, typescript, python, go, swift`) is a
  hand-written struct in `crates/kenn-config/src/language/<lang>.rs` with its own
  `command: Vec<String>`, a `default_<lang>_command()`, and a hand-written
  `Default`. `LanguageConfig` (`language/mod.rs:31-54`) aggregates them under
  `#[serde(deny_unknown_fields)]`. `Config::indexing_signature`
  (`config.rs:86-90`) hashes `serde_json(self.language)`, so any field added to
  these structs automatically joins the reindex key. `Config::validate`
  (`config.rs:96-108`) rejects an empty `command`.
- **Spawn**: every driver builds `Command::new(command[0])`, appends
  `command[1..]`, then appends its own intrinsic args. None call
  `.current_dir()`. The intrinsic args are **absolute host paths**:
  - `rust` — `scip <ws_root> --output <abs>`, env `CARGO_TARGET_DIR=<abs>`
  - `go` — `index --module-root <abs> --output <abs>`
  - `python` — `index --cwd <ws_root> --output <abs> [--target-only <abs>]`
  - `dotnet` — `index --workspace <ws_root> --projects <abs>…`, stdout→`<abs>`
  - `swift` — `index --workspace <ws_root> --projects <abs>…`
  - `typescript` — `index --workspace <ws_root> --tsconfigs <relative>…` (the
    one driver that forwards workspace-relative project paths)
- **Funnel**: `configure_runner` (`workflow.rs:220-278`) is the single place that
  reads `config.language.<lang>.command` and clones it into each driver struct.
  It is shared by the CLI and MCP/workflow paths.
- **Ingest**: `ingest.rs:287` seeds `project_root_uri` from kenn's workspace
  root; `absorb_scip_metadata` (`ingest.rs:251`) overrides it with the SCIP
  `Metadata.project_root`. A document whose canonicalized path is not under
  kenn's root produces `CanonicalizeError::OutsideRoot` (`canonicalize.rs:307`),
  which the ingest handler swallows with `return Ok(())` (`ingest.rs:301-307`) —
  no counter, no report entry.
- **Staleness**: `compute_staleness_key` (`staleness.rs:117`) = `Git { head,
  dirty_files (relative paths), config_sig }` or `Tree { fingerprint, config_sig
  }`. No absolute path; compared only within one workspace.
- **Release**: no `.github/`, no kenn Dockerfiles, no publish recipe. Sidecars
  are native single-file host binaries built by `just build-indexer-*` into
  `./build/` and installed by `just install`.

## Decisions

### D1 — Mount the workspace at its own absolute path on POSIX; translate only on Windows

The drivers already pass absolute host paths everywhere. If the container
bind-mounts the host workspace **at the identical absolute path**
(`-v /host/ws:/host/ws -w /host/ws`), then:

- every absolute-path argument (`<ws_root>`, `--output <abs>`, `--module-root`,
  `CARGO_TARGET_DIR`, …) is valid *inside* the container with no rewriting;
- the SCIP `project_root` the tool emits is that same host path, so
  `strip_prefix` in `ingest.rs` succeeds and nothing lands in `OutsideRoot`.

This means the POSIX docker path touches **zero drivers** — the whole change is
localized to the launcher-token rewrite in `configure_runner` (D4). That is the
decisive reason to prefer it over a uniform `/work` mount.

Windows breaks the trick: `C:\Users\x\repo` is not a valid Linux container path,
so a Windows host needs a fixed POSIX mount root (`/work`) plus translation of the
driver path arguments and the incoming SCIP `project_root`. That is a **separate
follow-on change** — this change ships POSIX (macOS/Linux) only. The launcher
rewrite is kept behind a `MountStrategy` seam (`SamePath` now, `Translate` later)
so the Windows path drops in without reworking the POSIX path.

- **Alternative considered — uniform `/work` + always translate.** One code
  path, but it forces the translation layer (and its per-driver arg rewriting)
  onto macOS/Linux where it buys nothing, and it re-introduces exactly the
  host↔container `project_root` mismatch that D2 must defend against. Rejected as
  the default; the future Windows change adopts it out of necessity.

### D2 — `OutsideRoot` becomes a counted, reported outcome (foundation, ships first)

Independent of Docker, `ingest.rs:301-307` must stop swallowing `OutsideRoot`.
The handler will increment a dropped-document counter and record a `RunReport`
entry naming the mismatch (`project_root` vs kenn root). Two severities:

- **Drops occurred but in-root docs survived** → warn on the drops (partial
  index; the rest is still useful).
- **Drops occurred and nothing survived** — `out_of_root_drops > 0` **and** the
  run's total in-root document count is `0` → **fail with a non-zero exit**, so a
  scripted `init && index` stops instead of proceeding on an empty index.

The gate is deliberately `drops > 0 && in_root == 0`, evaluated at the **run**
level, so the three honestly-empty cases stay successes: an empty repo, an
all-`Excluded` repo, and a producer that emitted nothing all have zero drops; and
a multi-producer run where one SCIP all-drops but another yields in-root docs has
`in_root > 0`, so it warns rather than fails. The tripwire only fires when the run
did produce documents and every one of them fell outside the root.

It is an *invariant tripwire*, not an expected outcome: on the POSIX same-path
mount (D1) `project_root` equals kenn's root, so it can't-happen; it fires only
when the mount/translation is wrong, the workspace root is a symlink the tool
canonicalizes away, or a future Windows translation is buggy. Today that same case
silently emits an empty index and exits 0 — the bug this fixes. It ships and is
verifiable before any container code exists.

Note `Excluded` stays a silent skip — exclusion is intentional; `OutsideRoot` is
not.

### D3 — `runtime` + `image` per language; `command` stays the in-container argv

Add to each of the six code-driver structs:

```toml
[language.rust]
runtime = "docker"                    # "local" (default) | "docker"
image   = "ghcr.io/kenn/rust-analyzer@sha256:…"   # OCI ref, digest-pinned
command = ["rust-analyzer"]           # unchanged: the argv run *inside* the image
```

`command` keeps its exact current meaning (the tool + flags), so `validate`'s
non-empty check is unchanged and a language can flip host↔docker without
rewriting its command. `runtime`/`image` are added field-by-field to the six
structs and their `Default` impls (`runtime = "local"`, `image = None`) — there
is no shared base to add them to once. Because `indexing_signature` hashes the
whole `[language.*]` block, flipping `runtime` or bumping `image` invalidates the
staleness key automatically.

- **Alternative — reuse `command = ["docker","run",…]` verbatim.** No new schema,
  but the user hand-writes the mount + image + in-container argv, and gets the
  path-space wrong every time. Rejected: the whole value is kenn synthesizing the
  mount correctly.

### D4 — Rewrite launcher tokens in `configure_runner`; keep drivers path-agnostic

`configure_runner` is the single funnel. When `runtime = "docker"`, it wraps the
configured `command` into the docker invocation before handing it to the driver:

```
["rust-analyzer", …]  ->  ["docker","run","--rm",
                           "--user","<uid>:<gid>","-e","HOME=/tmp",
                           "-v","/host/ws:/host/ws","-w","/host/ws",
                           "-e","CARGO_HOME=/kenn-cache/cargo",   # deps, see D8
                           "-v","<host-cache>:/kenn-cache",
                           "<image>","rust-analyzer", …]
```

The driver is unchanged: it still does `Command::new(command[0])` (`docker`) and
appends its absolute-path intrinsic args, which — under D1 — are valid in the
container. The mount spec is computed once (it needs the workspace root, which
`configure_runner` has) and carried on the driver alongside `command`.

The one wrinkle is the output/`.kenn` path: `--output <abs>` writes inside the
mount, so the SCIP lands on the host automatically. `CARGO_TARGET_DIR` similarly
resolves under the mount. TypeScript's relative `--tsconfigs` also just works
(relative to `-w /host/ws`).

**File ownership.** The container writes the SCIP output and scaffolds `.kenn/` on
the host bind mount; left as the image's default root user, those files land
**root-owned**, and the next `kenn` run (as the invoking user) cannot overwrite
them. So the invocation pins `--user <uid>:<gid>` to the caller's ids and sets a
writable `HOME=/tmp` (the image cannot assume a writable `/root`). Because a
non-root uid also cannot write a root-owned Docker *named* volume, the package
caches (D8) are **host-directory bind mounts** (host-owned, writable by the uid),
not named volumes, and each tool's cache location is pointed at them by an
explicit env var (`CARGO_HOME`, `GOMODCACHE`, …) rather than a `HOME`-relative
default.

### D5 — Content-addressed SCIP-output cache — **DROPPED** (was: key on `(image digest, config_sig, HEAD)`)

The original plan cached the SCIP *output* keyed on `(image_digest, config_sig,
staleness_key)` and, on a key hit, ingested the stored `.scip` instead of running
the container. Design review dropped it. `6.1` (the key helper) was implemented
and then reverted; `6.2` was never built.

**Why dropped.** Caching the output requires *kenn* to own invalidation — to
decide exactly which inputs feed a language's indexer and hash them. Two viable
keys, neither worth it:

- **Whole-tree key** (`staleness_key` = HEAD + dirty files): safe (a byte-identical
  tree yields a byte-identical `.scip`), but any change to *any* file invalidates
  *every* language's entry, and an unchanged tree is already short-circuited by
  git-aware staleness-skip before the cache is consulted. So it only helps the
  re-index-the-identical-tree case (CI shards/re-runs, an extra clone, `--force`),
  and even that needs the shared cache dir persisted. Marginal.
- **Per-language source key** (hash only that language's inputs): would help the
  polyglot inner loop (edit Rust → reuse the C#/TS `.scip`), but the input set is
  **not reliably determinable**. An indexer reads more than its language's source:
  lockfiles, resolved external deps, and arbitrary files via
  `include!`/`//go:embed`/source-generators. **Under-hashing → a false hit → a
  silently wrong index** — a worse UX than no cache. The only over-approximation
  guaranteed not to under-hash is the whole tree (the coarse key above).

**What we keep instead (D8 / task 3.4).** Cache the **downloaded deps** and
**build intermediates** (`CARGO_HOME`, `GOMODCACHE`, `target/`, …). There the
*toolchain* owns invalidation — cargo checks `Cargo.lock` + per-crate
fingerprints; incremental compilation knows what to rebuild — so kenn never
guesses a file set. It is safe by construction and is the real speedup: the
indexer still runs, but warm.

### D6 — One Dockerfile per indexer, a GitHub Actions workflow, digest pins

Greenfield. Add:

- A Dockerfile per indexer image under a top-level `docker/`, in two shapes: the
  three **built sidecars** (`kenn-dotnet`, `kenn-ts`, `kenn-swift`) copy the
  artifact `just build-indexer-*` already produces; the three **external tools**
  (`rust-analyzer`, `scip-go`, `scip-python`) install the upstream release. (The
  sidecars live under `indexers/`; the external tools have no such dir, hence the
  shared top-level `docker/`.)
- `.github/workflows/images.yml` — build + push `ghcr.io/kenn/<name>` on release
  tags, emitting the resulting digests.
- `just` recipes (`build-image-*`, `push-images`) mirroring the existing
  `build-indexer-*` recipes, for local iteration.

`kenn init --docker` and the defaults reference images **by digest**, not tag, so
the cache (D5) and reproducibility hold even if a tag is later re-pushed.

### D7 — `kenn init --docker` opts a degraded language into its image

`init` already probes each language's toolchain and reports degraded ones with an
install hint (`init/detect.rs`). With `--docker`, a degraded language whose image
is published is instead written as `runtime = "docker"` + the default image, and
reported as `containerized` rather than `degraded → text`. Consistent with the
shipped non-interactive UX: a report, not a prompt. Absent `docker` on `PATH`,
`--docker` is a reported error, and detection falls back to the existing degrade.

### D8 — Persistent per-ecosystem package caches

A container carries the toolchain but **not** the foreign repo's third-party
dependencies; the indexer resolves them at run time (the dogfood's isolated
rust-analyzer run spent 3+ minutes doing exactly this before timing out). To
avoid re-fetching on every index, `configure_runner` bind-mounts a **kenn-managed
host cache directory** into the container at `/kenn-cache` and points each tool's
cache location there with an explicit env var (so it works regardless of the
container `HOME`, and — being a host bind mount — stays writable under D4's
`--user`):

| language      | cache env var → container path        |
| ------------- | ------------------------------------- |
| rust          | `CARGO_HOME=/kenn-cache/cargo`        |
| go            | `GOMODCACHE=/kenn-cache/go`           |
| python        | `PIP_CACHE_DIR` / `UV_CACHE_DIR=/kenn-cache/py` |
| typescript    | `npm_config_cache` / bun `/kenn-cache/ts` |
| dotnet        | `NUGET_PACKAGES=/kenn-cache/nuget`    |
| swift         | swift package cache `/kenn-cache/swift` |

The first index of a repo populates the host cache dir; later indexes — and other
repos in the same ecosystem — reuse it. This is orthogonal to the SCIP output
cache (D5): **D8 speeds a cache *miss*** (deps are warm), **D5 skips the run
entirely on a *hit***. Because the cache is a kenn-owned host directory (not a
Docker named volume), kenn can bound it with an LRU alongside the SCIP cache
rather than leaning on `docker volume prune`.

## Risks / open questions

- **Cache growth across three caches.** The SCIP output cache (D5), the vector
  cache (`generation.rs:186`, already flagged as unbounded when several
  workspaces share one `[vectors] location`), and the D8 package cache all grow.
  D5 and D8 are both kenn-owned host directories, so each gets a bounded LRU keyed
  on a directory disjoint from the vector cache; the vector-cache GC is untouched.
- **`--user` image compatibility.** Running as the caller's uid with `HOME=/tmp`
  (D4) assumes each image's tool runs as an arbitrary non-root user and writes
  only to the workspace mount and `/kenn-cache`. The kenn-owned images (D6) must
  be built to honor this; verify per image in the image fixture run.
- **Driver cwd coupling.** No driver sets `.current_dir()`; a *relative*
  `command` (this repo uses `["build/kenn-ts"]`) resolves against the process
  cwd, which `-w` already decoupled from the workspace. A docker `command`
  is an image-internal argv so this does not bite the container path, but the
  interaction with a relative host `command` under `-w` should be spelled out.
- **Daemon detection latency.** Probing `docker info` on every run is slow;
  decide whether availability is probed once per `configure_runner` or cached.
- **SCIP `project_root` canonicalization.** Even under D1, a tool that
  canonicalizes symlinks could emit a `project_root` that differs from kenn's
  root; D2's reporting is what makes such a case visible rather than silent.
- **Windows scope (decided).** Windows path translation is a **separate
  follow-on change**; this change ships POSIX only. The `MountStrategy` seam is
  retained so `Translate` drops in without reworking the POSIX path.
