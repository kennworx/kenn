## Why

When an indexer cannot run, the diagnostic that would fix it usually already
exists — and kenn throws it away.

`kenn init` probes each indexer with `--version` and keys only on the exit
status, discarding the process's own explanation:

```rust
// crates/kenn-cli/src/init/detect.rs:251
.stderr(std::process::Stdio::null())
```

So the user sees a fixed per-language hint no matter what failed. The
motivating case makes the cost concrete. `kenn-swift` links
`@rpath/libIndexStore.dylib` as a **hard** dependency — no `weak` marker,
unlike the Swift runtime libraries beside it — so when it cannot be resolved,
dyld aborts the process before `main` and prints:

```
dyld[…]: Library not loaded: @rpath/libIndexStore.dylib
```

That message names the exact problem. kenn replaces it with "install the Swift
toolchain", which is wrong: the toolchain is installed, a library from it is
not on the load path.

Index time discards differently. Every driver captures stderr, but only the
third-party SCIP drivers extract from it:

| driver | extraction | what the caller gets |
|---|---|---|
| rust, go, python (third-party) | `error_reason` | one actionable line |
| ts, dotnet, swift (kenn's own) | none | the raw 8 KB tail |

`error_reason` picks the first `error`-prefixed line rather than the last —
the last is usually a backtrace frame — and it is wired only to the tools that
cannot be made to follow a kenn convention. kenn's own sidecars get their whole
tail dumped into `failed_projects`. For an agent reading a failure over MCP,
one useful sentence inside 8 KB of build noise is the problem.

## What Changes

**kenn stops discarding diagnostics.**

- `kenn init`'s probe captures stderr and reports what the indexer (or the
  dynamic loader) actually said, preferring it over the static hint.
- A failing probe distinguishes "could not execute" from "executed and failed".
- The sidecar path at index time leads with the extracted `error:` line and
  keeps the tail after it.

**Explicitly NOT in scope: requiring sidecars to emit new diagnostics.** A
first version of this change specified that. It does not survive contact with
the two failure modes:

- **A missing linked library** — the process never starts, so it cannot print
  anything. Only the parent can report it.
- **A missing toolchain** — `--version` must still succeed. `just probe-smoke`
  already asserts exactly that ("the probe must never need the toolchain it
  probes"), so requiring `--version` to emit an install error would contradict
  a contract already enforced.

Sidecar-emitted messages for failures a sidecar *can* detect at index time are
a separate question, worth revisiting once relaying lands and we can see what
is still missing rather than guessing.

## Capabilities

### New Capabilities
- `indexer-diagnostics`: how kenn surfaces a failing indexer's own explanation
  to its caller, at probe time and at index time.

### Modified Capabilities
- `workspace-init`: `kenn init`'s degraded report names a fixed install hint
  per language. It must prefer the failing indexer's own diagnostic.

## Impact

**Code**
- `crates/kenn-cli/src/init/detect.rs` — `probe_ok` and `Availability::Degraded`
- `crates/kenn-cli/src/init/report.rs` — the degraded line
- `crates/kenn-indexer/src/pipeline/ingest.rs` — `record_jsonl_exit_status`
- `crates/kenn-indexer/src/driver/contract.rs` — `error_reason` gains a second
  caller; unchanged itself

**Not affected** — the sidecars. No indexer changes behaviour; this is entirely
about kenn no longer dropping what they already say. `kenn-dotnet`'s
`MsBuildBootstrap.LocatorAdvice` is the standard to aim at, and it already
exists.
