> Narrowed after implementation began. Requiring sidecars to emit new
> diagnostics was dropped: a missing linked library aborts the process before
> `main` (so it cannot print), and a missing toolchain must leave `--version`
> succeeding — which `just probe-smoke` already enforces. What remains is kenn
> no longer discarding the diagnostics that already exist.

## 1. Capture the diagnostic at probe time

- [x] 1.1 Change `probe_ok` (`init/detect.rs:244`) to return whether the
  command executed, its exit status, and its stderr — instead of a bool.
  `--version` output is small, so `output()` suffices; no background drain is
  needed here, unlike the index-time path where the child streams. → verify:
  `probe_ok_reflects_exit_status` still passes.
- [x] 1.2 Carry the message on `Availability::Degraded`, ALONGSIDE the existing
  static `hint` rather than replacing it — third-party indexers produce no
  message and the hint is all they have. → verify:
  `a_failing_probe_degrades_with_command_and_hint` still passes.
- [x] 1.3 Render in `init/report.rs`: prefer the indexer's message over the
  static hint, and distinguish "not found" from "ran and failed". → verify: a
  deliberately broken sidecar shows its own text in `kenn init`.
- [x] 1.4 **Mutation-check the relay**: discard stderr in `probe_ok` again and
  confirm the new test goes RED. A test that only checks capture would pass
  while the render still shows the generic hint. → verify: red on the
  mutation, green on restore.

## 2. Extract at index time

- [x] 2.1 `record_jsonl_exit_status` (`pipeline/ingest.rs:619`) appends the raw
  8 KB tail with no extraction — `error_reason` is wired only to the SCIP
  drivers. Apply it here, leading with the `error:` line and KEEPING the tail
  below it. → verify: a failing sidecar's `failed_projects` entry opens with
  the actionable line and still contains the tail.
- [x] 2.2 **Mutation-check**: remove the extraction so the entry leads with raw
  output again, and confirm the test goes RED. An assertion that greps the
  whole entry for the message passes either way and guards nothing. → verify:
  red on the mutation, green on restore.

## 3. Documentation

- [x] 3.1 README: a degraded language reports the indexer's own reason, so
  "reinstall it" is not the universal answer.

## 4. Verify the motivating case

- [x] 4.1 Stage an unresolvable `libIndexStore` and confirm `kenn init` reports
  the loader's message naming that library, rather than claiming the Swift
  toolchain is missing. → verify: the report names `libIndexStore`.
