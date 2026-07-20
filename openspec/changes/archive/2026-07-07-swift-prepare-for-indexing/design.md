# swift-prepare-for-indexing — design

## Context

`ensureSwiftPMStore` (`Provisioning.swift:41-50`) runs `swift build --build-tests`; on failure it logs to stderr and reads whatever store exists. The reader (`Indexer.swift:76-84`) iterates all units with no freshness check. Spike evidence (Swift 6.3.2, `tmp/spike-swift/`, `tmp/spike-*.log`):

| Scenario | Result |
|---|---|
| Type/syntax error, single target, plain build | exit 1, but units+records emitted for **all** files incl. the broken one |
| Broken dep target `LibA`, plain build | `AppB` gets **no unit** (dependents skipped) — the real gap |
| `-Xswiftc -continue-building-after-errors` | does **not** rescue dependent targets |
| `swift build --experimental-prepare-for-indexing` | **exit 0**, all targets compile, store at standard path, unmodified kenn-swift reads it fully (broken file + dependent both indexed, edges intact) |
| Edit dependent while dep stays broken, re-run prepare | new symbol indexed — freshness holds |
| prepare + `--build-tests` | composes, exit 0 |

## Goals / Non-Goals

**Goals:**
- SwiftPM index coverage independent of whether the target compiles.
- Never emit symbols/ranges from units older than their source (stale-store hazard).
- Transparent fallback on toolchains lacking the experimental flag.

**Non-Goals:**
- Xcode-mode error tolerance beyond staleness skip + failure reporting (no xcodebuild prepare equivalent exists).
- Per-file re-indexing of stale units (that's SourceKit-LSP's job, not a batch indexer's).
- New config surface — no `kenn.toml` knob; behavior + fallback only.

## Decisions

### D1 — Prepare-first, plain-build fallback, then existing-store fallback

`ensureSwiftPMStore` tries `swift build --experimental-prepare-for-indexing --build-tests`; if it exits non-zero (unknown option on pre-6.x toolchains, manifest evaluation failure, resolution failure), it runs the current `swift build --build-tests`; if that also fails it emits the build-failure error frame (from `index-status-error-reporting`) and reads any existing store. Trying prepare first rather than only-on-failure because the spike showed it's *faster* (skips codegen) and strictly more tolerant — there's no case where the plain build succeeds and prepare fails that the fallback doesn't cover. Alternative — gating on a toolchain version probe — adds a `swift --version` parse for no benefit over just trying the flag.

### D2 — Staleness handling depends on how the store was provisioned

Three modes (review finding: an unconditional skip emptied the index on trusted-store reads whenever a fresh checkout postdated the store — e.g. a cloned repo plus a CI-built artifact):

- **skip** — after a FAILED in-process build: the store is a fallback read; stale units' ranges describe code that no longer exists, so drop them and report. Emitting them would give `get_source` wrong spans (fresh `content_hash` paired with stale ranges). **Ratio guard** (second review): when strictly more than half of the units whose source still exists are mtime-stale, keep them and report the skew instead — mass staleness is a checkout/cache mtime signature (fresh clone over a CI-cached store, archive/rsync/Docker restore), not real edits, and no content baseline exists in the store to tell touched from changed. Missing-source units are skipped in EVERY checking mode, including warnOnly (third review: emitting one pairs an empty-bytes content_hash with ranges into a gone file), and are excluded from the ratio denominator. The skip-mode classification runs as a single-parse pre-pass that buffers the fields ingest needs, so UnitReaders are never constructed twice.
- **warnOnly** — `--skip-build` / `--store`: the caller trusts the store; keep stale units, report the count. Skipping here is a total blackout, not "partial coverage".
- **off** — after a successful in-process build: every unit is fresh by construction; the check (two stats per unit) is pure waste, so it doesn't run.

The warning frame is `severity:"warning", source:"store"` — matches D2 severity policy in `index-status-error-reporting`. If the store's `v5/units` directory is absent (layout change), the reader warns once and disables the check instead of silently treating everything as fresh.

### D3 — Freshness rule is mtime-based, strict-newer, memoized

A unit is stale iff its main source is STRICTLY newer than the unit file — equal mtimes are fresh on purpose (on a coarse-mtime filesystem a source written and compiled in the same clock tick would otherwise be dropped right after a successful build). Mtimes come from bare `stat(2)` (not `FileManager.attributesOfItem`, which builds a full attribute dictionary per call) and source mtimes are memoized per file across units. No content hashing of the store side is possible (units don't carry source hashes). A `touch`ed-but-unchanged file false-positives as stale — acceptable in `skip` mode (only reached after a failed build) and harmless in `warnOnly` (kept anyway).

### D4 — Flag detection is exit-code-only

No stderr parsing to distinguish "unknown option" from other failures. Any prepare failure → plain-build fallback. Simpler, and the plain build's own failure handling (error frame + existing-store read) already covers the residue.

## Risks / Trade-offs

- [Experimental flag changes name/semantics in a future toolchain] → D1 fallback keeps indexing working (at today's quality); the flag is load-bearing for sourcekit-lsp's default-on background indexing since Swift 6.1, so removal without replacement is unlikely.
- [Prepare-mode partial swiftmodules omit function bodies → fewer `calls` edges?] → Spike showed the call edge `use() → value()` survived; but spike bodies were trivial. Verify edge counts on a real package (kenn-swift's own test fixture) before/after as part of the change's verification tasks.
- [Double build cost on old toolchains (prepare attempt fails, then plain build)] → the failed attempt on unknown-option exits in <1s (argument parse error, no compilation).
- [mtime granularity / clock skew on network filesystems] → same exposure sourcekit-lsp accepts; not worth content-hash plumbing.

## Open Questions

- ~~Should `--skip-build` also skip the staleness check (user says "trust the store")?~~ Resolved by review: trusted-store reads (`--skip-build`/`--store`) keep stale units and warn (`warnOnly` mode) — skipping them emptied the index whenever a fresh checkout postdated the store (git clone stamps sources with checkout time). See D2.
