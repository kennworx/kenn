## 1. Make the contract testable before anything emits it

- [ ] 1.1 Extend `just probe-smoke` to assert each kenn-authored sidecar emits
  an `error:` line naming an install command when run against an unreachable
  toolchain (D4). Land it FIRST, failing, so the contract is enforced before it
  is claimed. → verify: the recipe fails naming all three sidecars.

## 2. Emit the diagnostic from each sidecar

- [ ] 2.1 `kenn-swift`: detect that `libIndexStore` cannot be resolved and emit
  `error: <library> not found; install with: <command>`. This is the case that
  motivated the change — the binary currently installs fine and dies later
  naming neither the library nor the reason. → verify: probe-smoke passes for
  swift; stdout stays empty.
- [ ] 2.2 `kenn-dotnet`: same shape for a missing/unusable SDK. It already
  names the `global.json` pin rather than blaming PATH — keep that specificity
  and add the install command. → verify: probe-smoke passes for dotnet.
- [ ] 2.3 `kenn-ts`: same shape. → verify: probe-smoke passes for typescript.
- [ ] 2.4 Confirm no sidecar writes any diagnostic to stdout. → verify: run
  each against a broken toolchain and assert stdout is empty or valid JSONL —
  a stray line here is a wire-corruption bug, which is how a `traceResolution`
  line once surfaced as a line-1 parse error.

## 3. Capture it at probe time

- [ ] 3.1 Change `probe_ok` (`init/detect.rs:244`) to return success plus
  captured stderr instead of a bool. `--version` output is small, so `output()`
  suffices — no background drain needed, unlike the index-time path. → verify:
  `probe_ok_reflects_exit_status` still passes.
- [ ] 3.2 Carry the message on `Availability::Degraded`, alongside the existing
  static `hint` rather than replacing it — third-party indexers have no
  message and the hint is all they have. → verify:
  `a_failing_probe_degrades_with_command_and_hint` still passes.
- [ ] 3.3 Render it in `init/report.rs`, preferring the indexer's message over
  the static hint, and distinguish "could not execute" from "executed and
  failed" (D3). → verify: `kenn init` against a workspace with a deliberately
  broken sidecar shows the sidecar's own text.
- [ ] 3.4 **Mutation-check the relay**: make `probe_ok` discard stderr again and
  confirm the new test goes RED. Capturing stderr and then not rendering it
  would pass a test that only checks the capture. → verify: red on the
  mutation, green on restore.

## 4. Index time

- [ ] 4.1 Confirm `error_reason` already surfaces a conforming line end-to-end
  — it prefers the first `error`-prefixed line by design. This is likely a
  no-op in code and a test. → verify: an indexer failing mid-run puts its
  `error:` line in the run report's `failed_projects`, not a backtrace frame.

## 5. Documentation

- [ ] 5.1 Document the contract where a sidecar author will see it — one line
  on the stream, the prefix, and that stdout is the wire.
- [ ] 5.2 README: note that a degraded language reports the indexer's own
  reason, so "reinstall it" is not the universal answer.

## 6. Verify the case that motivated this

- [ ] 6.1 On a machine with only the Command Line Tools — NOT the development
  machine, which has Xcode and therefore cannot reproduce it — install
  `kenn-swift`, run `kenn init` in a Swift workspace, and confirm the report
  names `libIndexStore` and a working install command. A staged failure proves
  the formatting; only the real one proves the detection.
