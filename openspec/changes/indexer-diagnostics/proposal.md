## Why

When an indexer cannot run, the person who has to fix it is told the least
useful version of why.

`kenn init` probes each indexer with `--version` and keys only on the exit
status — it discards the process's own explanation:

```rust
// crates/kenn-cli/src/init/detect.rs:251
.stderr(std::process::Stdio::null())
```

So a user sees a fixed, generic hint attached to the language, regardless of
what actually failed. `kenn-swift` failing because `libIndexStore.dylib` cannot
be resolved and `kenn-swift` failing because the binary is corrupt produce
identical output. The sidecar knows which dependency is missing; kenn throws
that away and guesses.

This is the same failure shape this repo keeps rediscovering — a component that
exits non-zero for a specific, nameable reason, reported as something vague.
`kenn-swift`'s missing `libIndexStore` is the current example: the binary
installs fine and dies later "naming neither the library nor the reason".

Index time fails the caller differently, and the split is the opposite of what
you would guess:

| driver | extraction | what the caller gets |
|---|---|---|
| rust, go, python (third-party) | `error_reason` | one actionable line |
| ts, dotnet, swift (kenn's own) | none | the raw 8 KB stderr tail |

`error_reason` picks the first `error`-prefixed line rather than the last —
because the last is usually a backtrace frame — and it is applied ONLY to the
third-party tools, which cannot be made to follow a kenn convention. kenn's own
sidecars, the ones that could, get `record_jsonl_exit_status` dumping their
whole tail verbatim into `failed_projects`.

So the extraction convention exists and is proven, and is wired precisely where
it cannot be relied on. For an agent reading a failure over MCP, 8 KB of build
noise around one useful sentence is the problem.

## What Changes

**Sidecars emit an actionable diagnostic.** When a kenn-authored indexer
(`kenn-swift`, `kenn-dotnet`, `kenn-ts`) cannot run, it writes a line to
**stderr** beginning with `error:` that names both the missing dependency and
the command that installs it, then exits non-zero.

Stderr specifically, and never stdout: stdout is the JSONL wire, and a
diagnostic written there corrupts a frame and surfaces as a parse error
blaming the indexer's output format.

**`kenn init` captures and relays it.** `probe_ok` grows from a bool into a
result carrying the probe's stderr. A failing probe reports the sidecar's own
message in preference to the built-in hint, which stays as the fallback for a
sidecar that said nothing useful (or was never executed, e.g. not found).

**The `error:` prefix becomes a contract** rather than a coincidence that
`error_reason` happens to rely on. It is what lets one line be picked out of
build noise at index time, and it is already load-bearing there.

## Capabilities

### New Capabilities
- `indexer-diagnostics`: what a kenn-authored indexer must print when it cannot
  run, on which stream, and how kenn surfaces that to the caller — at probe
  time and at index time.

### Modified Capabilities
- `workspace-init`: `kenn init`'s degraded report currently names a fixed
  install hint per language. It must prefer the failing indexer's own
  diagnostic when there is one.

## Impact

**Code**
- `crates/kenn-cli/src/init/detect.rs` — `probe_ok` and the `Degraded`
  variant, which currently carries only `{ command, hint }`
- `crates/kenn-cli/src/init/report.rs` — renders the degraded line
- `indexers/kenn-swift`, `indexers/kenn-dotnet`, `indexers/kenn-ts` — emit the
  diagnostic
- `crates/kenn-indexer/src/driver/contract.rs` — `error_reason` already does
  the right thing; this makes the convention it depends on explicit

**Not affected** — third-party indexers. `rust-analyzer`, `scip-go` and
`scip-python` cannot be made to follow a kenn convention, so their built-in
hints remain the only thing available and the fallback path must stay.

**Test surface** — `just probe-smoke` already asserts every sidecar handles an
unreachable toolchain without crashing. It becomes the natural place to assert
that they *say something useful* while doing so, which is the part it does not
currently check.
