## 1. Raw record schema and store layout

- [x] 1.1 Define the raw JSONL record types as a serde-tagged enum (`#[serde(tag = "kind")]`) with variants `session_start`, `prompt`, `touch`, `session_end`, in a new module of the appropriate kenn crate (likely kenn-cli or a small new `kenn-cc-hook` crate; decide during implementation based on dependency direction)
- [x] 1.2 Extend the kenn store layout to add `history/raw/` and `history/ready/` slots, exposed via accessors next to the existing `findings/` accessors
- [x] 1.3 Confirm with `vector-store-layout-cleanup` owners that the `history/` slot does not conflict with the active layout change; coordinate ordering if it does
- [x] 1.4 Update `.gitignore` rules so that `history/raw/` and `history/ready/` are gitignored by default (raw transcripts are user-local v1; sharing comes in a follow-up)

## 2. `kenn cc-hook` CLI subcommand

- [x] 2.1 Add a `cc-hook` subcommand to `kenn-cli` with subsubcommands `session-start`, `prompt`, `touch`, `session-end`
- [x] 2.2 Implement stdin JSON parsing for each event using a dedicated input struct per subsubcommand that extracts only the fields needed for the matching raw record kind
- [x] 2.3 Implement the `history/raw/<session_id>.jsonl` append as an `O_APPEND` open-write-close per call, so concurrent writes from parallel sessions cannot interfere
- [x] 2.4 Implement the `history/ready/<session_id>` zero-byte marker creation on `session-end`
- [x] 2.5 Implement graceful error handling: any recoverable error (bad JSON, missing dir, IO error) logs to a kenn diagnostic file and exits 0
- [x] 2.6 Pin the kenn diagnostic log path (likely `<store>/cc-hook.log` or `~/.cache/kenn/cc-hook.log`) and document it
- [x] 2.7 Unit-test each subsubcommand against representative hook JSON fixtures captured from a real Claude Code session

## 3. Hook installation helper

- [x] 3.1 Add a `kenn cc-hook install` subsubcommand that prints the required hook-config snippet to stdout by default
- [x] 3.2 Add a `--write` flag that merges the snippet into `~/.claude/settings.json`, preserving any pre-existing hooks for other events and leaving the file as valid JSON
- [x] 3.3 Validate that the printed snippet wires `SessionStart`, `UserPromptSubmit`, `PostToolUse` (matcher `Edit|Write|Read`), and `SessionEnd` to the corresponding `kenn cc-hook` calls

## 4. Latency benchmark

- [x] 4.1 Add a small benchmark that invokes `kenn cc-hook touch` with a realistic payload N times and reports p50/p95/p99
- [x] 4.2 Run the benchmark; record p95 and document it in the change's validation notes (target ≤5ms; trigger `async: true` fallback in docs if >10ms)

## 5. End-to-end validation

- [x] 5.1 Run `kenn cc-hook install --write` against a test `~/.claude/settings.json` (or a fixture path) and confirm a valid merged result
- [x] 5.2 Run a real Claude Code session that edits two files across at least five prompts; confirm `history/raw/<session_id>.jsonl` contains the expected records in order
- [x] 5.3 Confirm `history/ready/<session_id>` is created on session end
- [x] 5.4 Inspect the raw JSONL with `jq` and visually verify the schema matches the spec
- [x] 5.5 Force a malformed payload through `kenn cc-hook touch`; confirm exit 0 and a diagnostic log entry
- [x] 5.6 Run `just crap-ci` on the changed crates; address any over-threshold or regression entries on touched functions

## 6. Documentation

- [x] 6.1 Add a short "conversation capture (preview)" section to the kenn README: what it captures, how to opt in (`kenn cc-hook install`), what the trust boundary is, where the data lives, that this is preview / capture-only
- [x] 6.2 Document the raw record schema in a comment near the serde enum definition (the spec is the authoritative version, but a code-local reference is useful)
- [x] 6.3 Note the known gap: Bash-driven file mutations are not captured; this is the same gap the full design accepts

## Validation notes

- **Latency benchmark (task 4.2)** — measured on macOS arm64 with release build, 50 invocations after 5 warm-up runs, payload is a realistic `Edit` `PostToolUse` JSON: **p50 = 7.3 ms, p95 = 8.3 ms, p99 = 9.0 ms**. Over the ≤5 ms target but well under the ≤10 ms async-fallback threshold. The bulk of the time is Rust binary process spawn; the actual append work is sub-millisecond. Future optimization (smaller dep tree for cc-hook, LTO+strip, or a dedicated thin binary) can shrink this if needed. v1 ships with sync hooks per the install snippet.
- **Vector-store-layout-cleanup coordination (task 1.3)** — `grep -r history openspec/changes/vector-store-layout-cleanup/` returns no matches. The active layout change scopes to `runs/`, `lance/`, vector sidecars in the derived subtree; `history/` lives under `committed_root` and does not conflict.
- **Open items from §5** — 5.2/5.3/5.4 require a real Claude Code session against an installed snippet (out of scope for an automated CI run; integration smoke `cc_hook_smoke::end_to_end_session_capture_creates_records_and_marker` exercises the same code paths in-process). 5.6 (`just crap-ci`) is the user's call to run; the touched functions are small (cyclomatic ≤6 by inspection) and well-covered by the new tests.
- **Resolved during validation** — 5.3 confirmed against completed session `a4c2d6b7…` (ready marker present). 5.4 confirmed against live capture via the kenn plugin's `hooks/hooks.json`; the inspection surfaced a spec/impl mismatch on `prompt_idx` (spec required it on `prompt` and `touch`, impl omitted it). Resolved by dropping `prompt_idx` from the spec — prompt↔touch grouping is recovered at ingest time from monotonic `t` ordering, which is cheaper than reading back the JSONL on every hook fire to denormalize an index at capture time. Spec and design updated accordingly. 5.6 passed (`CRAP gate PASSED: no regressions, no new over-threshold functions`). 5.2 confirmed against this very session (`f8dbd185…`): 11 prompts, 10 touches across 3 distinct files (Edit + Read tool variants), all in chronological order with `old_string`/`new_string` populated on Edits and absent on Reads — matches the updated spec exactly.
