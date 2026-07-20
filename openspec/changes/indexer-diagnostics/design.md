## Context

Two moments matter, and they behave differently today.

**Probe time.** `kenn init` runs `<indexer> --version` through `probe_ok`,
which nulls stdout, stderr and stdin and returns a bool. A failure becomes
`Availability::Degraded { command, hint }`, where `hint` is a constant from
`SPECS` in `detect.rs`. Nothing the indexer said survives.

**Index time.** Every driver pipes stderr, and `spawn_stderr_capture` drains it
in the background so the child cannot block on a full pipe. What happens next
splits by driver family:

- **rust, go, python** — `error_reason` extracts one line, preferring the first
  beginning with `error` over the last non-empty one, with a comment explaining
  that cargo and rust-analyzer end in a backtrace so `lines().last()` returns
  `6: __pthread_cond_wait` and discards the cause.
- **typescript, dotnet, swift** — no extraction.
  `record_jsonl_exit_status` appends the whole 8 KB tail to the failure message
  verbatim.

The extraction convention is therefore already proven, and wired exclusively to
the three indexers that cannot be made to follow a kenn convention. The three
that could are the ones dumping raw output.

`kenn-dotnet` shows the message quality this change wants everywhere.
`MsBuildBootstrap.LocatorAdvice` refuses the default "install the SDK" advice
when the SDK *is* installed and a `global.json` pin is unsatisfiable, naming
the pin, its `rollForward`, and three concrete fixes. It is exactly right —
and carries no `error:` prefix, so the extraction convention would not select
it. Good message, outside the contract.

## Goals / Non-Goals

**Goals:**
- A failing kenn-authored indexer names the missing dependency and the command
  that fixes it.
- That reaches the user at probe time (`kenn init`) and at index time.
- One convention, honoured by both paths.

**Non-Goals:**
- Changing third-party indexer behaviour. Their built-in hints stay.
- Structured/machine-readable diagnostics. A line of text is what both call
  sites already consume; JSON here would be a second wire format to version.
- Making kenn *repair* anything. It reports the command; the user runs it.

## Decisions

### D1 — stderr, `error:`-prefixed, one line, then exit non-zero

Stderr because stdout is the JSONL wire. This is not hypothetical: a
`traceResolution` line on stdout from the TypeScript compiler already caused a
line-1 JSONL parse error that read as a kenn-ts bug.

The `error:` prefix because `error_reason` selects on it and because the
alternative — take the last line — is already documented as a trap that returns
a backtrace frame.

One line because both consumers render it inline: `kenn init`'s per-language
report and the run report's `failed_projects`. A multi-line diagnostic is
allowed to follow, but the first `error:` line must stand alone as the summary.

Shape:

```
error: <what is missing>; install with: <command>
```

### D2 — `probe_ok` returns the probe's output, not a bool

It becomes a small result type carrying success plus captured stderr. The
`Degraded` variant grows a field for the indexer's own message, distinct from
the static `hint`.

**Precedence:** the indexer's message wins when present; the static hint is the
fallback. The indexer knows *which* dependency is missing; the static hint can
only name the tool generically. For third-party indexers there is no message,
so the hint is all there is — which is why the fallback stays rather than being
replaced.

Capture must not change the exit-status semantics `probe_ok` already has, and
must not deadlock: `--version` output is tiny, so a plain `output()` is
sufficient and no background drain is needed here (unlike index time, where the
child streams).

### D3 — A missing binary and a broken binary are different, and must read differently

`probe_ok` currently collapses "command not found" and "command ran and failed"
into one bool. They want different messages: the first is *install the
indexer*, the second is *the indexer is installed but cannot run, here is what
it said*.

The current report line — `degraded → text fallback (<command> not runnable)` —
is accurate for both but useful for neither.

### D4 — The sidecar path leads with the `error:` line and keeps the tail

`record_jsonl_exit_status` currently appends the raw 8 KB tail. It SHOULD lead
with the extracted `error:` line and keep the tail after it — not replace one
with the other.

Both readers matter and want different things. An agent reading
`failed_projects` over MCP needs the one actionable sentence at the front; a
human debugging a broken toolchain wants the surrounding build output. Leading
with the line and retaining the tail serves both; dropping the tail would trade
one information loss for another.

### D5 — `just probe-smoke` asserts the message, not just the exit code

It already runs each sidecar against an unreachable toolchain and asserts no
crash. It gains an assertion that stderr carries an `error:` line naming an
install command.

Without that, D1 is a convention nothing enforces, and the first sidecar to
quietly stop emitting it would be found by a user rather than by CI.

## Risks / Trade-offs

**A diagnostic can be wrong.** An indexer that names the wrong dependency sends
the user to install something irrelevant — worse than the generic hint, because
it is specific and confident. Each sidecar's message must come from the actual
failure it caught, never from a guess about why a generic error occurred.

**The `error:` prefix is a string contract between four codebases** — Rust, C#,
TypeScript and Swift — with no shared type to enforce it. `probe-smoke` (D4) is
the only thing that keeps them aligned, so it is load-bearing rather than nice
to have.

**Testing the message means testing a failure that is hard to stage.** The
honest check for `kenn-swift` is a machine where `libIndexStore` genuinely
cannot be resolved. That is not the development machine, which has Xcode. A
test that stages the failure by other means proves the formatting, not the
detection — and this repo has a history of tests that passed while guarding
nothing.
