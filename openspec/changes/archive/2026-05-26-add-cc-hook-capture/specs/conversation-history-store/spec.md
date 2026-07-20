## ADDED Requirements

### Requirement: hook events are captured via a `kenn cc-hook` CLI

The kenn CLI SHALL expose a `cc-hook <event>` subcommand that accepts Claude Code hook JSON on stdin and appends a single raw record to a per-session JSONL inbox under the kenn store at `local/history/<session_id>.jsonl`. The subcommand SHALL support events `session-start`, `prompt`, `touch`, and `session-end`, mapped from the Claude Code hooks `SessionStart`, `UserPromptSubmit`, `PostToolUse`, and `SessionEnd` respectively. The subcommand SHALL NOT perform any embedding, LLM call, network call, or write outside the kenn store — its only side effect is appending JSONL and (on `session-end`) writing a marker file.

#### Scenario: a tool-use hook appends a touch record

- **WHEN** `kenn cc-hook touch` is invoked with `PostToolUse` JSON for an `Edit` of `src/foo.rs`
- **THEN** a new line is appended to `local/history/<session_id>.jsonl` carrying `kind: "touch"`, `tool_name`, `file_path`, `old_string`, `new_string`, `t`, and `session_id`
- **AND** the subcommand exits 0

#### Scenario: a user-prompt hook appends a prompt record

- **WHEN** `kenn cc-hook prompt` is invoked with `UserPromptSubmit` JSON
- **THEN** a new line is appended to `local/history/<session_id>.jsonl` carrying `kind: "prompt"`, `prompt_text`, `t`, and `session_id`

#### Scenario: a session-start hook appends a session_start record

- **WHEN** `kenn cc-hook session-start` is invoked with `SessionStart` JSON
- **THEN** a new line is appended to `local/history/<session_id>.jsonl` carrying `kind: "session_start"`, `t_start`, `user`, `branch`, `git_sha`, and `cwd`

### Requirement: SessionEnd additionally writes a ready marker

`kenn cc-hook session-end` SHALL write a zero-byte marker file at `local/history/ready/<session_id>` in addition to appending the `session_end` record to the raw JSONL. The marker SHALL NOT be required by anything in this change; it exists so future ingest passes can enumerate finished sessions without scanning every raw inbox file.

#### Scenario: SessionEnd writes both the record and the marker

- **WHEN** `kenn cc-hook session-end` is invoked
- **THEN** a new line with `kind: "session_end"` is appended to `local/history/<session_id>.jsonl`
- **AND** a zero-byte file is created at `local/history/ready/<session_id>`
- **AND** the subcommand exits 0

### Requirement: the raw record schema is a tagged-union JSON line

Each line of `local/history/<session_id>.jsonl` SHALL be a single JSON object with a `kind` field whose value is one of `"session_start" | "prompt" | "touch" | "session_end"`. The remaining fields per kind SHALL be:

- `session_start`: `t_start`, `session_id`, `user`, `branch`, `git_sha`, `cwd`
- `prompt`: `t`, `session_id`, `prompt_text`
- `touch`: `t`, `session_id`, `tool_name`, `file_path`, `old_string?`, `new_string?`

Note: prompt↔touch grouping is recovered at ingest time from the monotonic `t` ordering (touches between adjacent prompt records belong to the preceding prompt). Carrying a denormalized `prompt_idx` at the hook layer would require each `kenn cc-hook` invocation to read back the JSONL to count prior prompts; the ingest pass scans the file once anyway, so the index is computed there.
- `session_end`: `t_end`, `session_id`

A future consumer SHALL be able to skip unknown `kind` values without error, and the schema SHALL be extensible by adding new `kind` values in later changes without breaking existing readers.

#### Scenario: all four kinds round-trip through a JSON parser

- **GIVEN** a `local/history/<session_id>.jsonl` containing one record of each kind
- **WHEN** the file is read line-by-line and each line is JSON-parsed
- **THEN** every line parses successfully
- **AND** the `kind` field selects the expected schema variant

### Requirement: the CLI fails silently on recoverable errors

`kenn cc-hook` SHALL exit with status 0 on any recoverable error — malformed input JSON, missing store directory, disk-full or permission errors on append. Any such error SHALL be appended to a kenn diagnostic log file but SHALL NOT propagate as a non-zero exit code that could interrupt the user's Claude Code session. Programming errors (panics) are out of scope of this requirement.

#### Scenario: a malformed hook payload does not break the session

- **WHEN** `kenn cc-hook touch` is invoked with stdin that is not valid JSON
- **THEN** the subcommand exits 0
- **AND** no raw record is written
- **AND** a diagnostic is appended to the kenn log

#### Scenario: a missing store directory does not break the session

- **WHEN** `kenn cc-hook prompt` is invoked but `local/history/` does not exist and cannot be created
- **THEN** the subcommand exits 0
- **AND** a diagnostic is appended to the kenn log

### Requirement: the hook latency budget is documented and benchmarked

The `kenn cc-hook` invocation SHALL complete in ≤5ms p95 under warm cache, measured by a benchmark that is part of the change. If the measured p95 exceeds 10ms in realistic conditions, the documented opt-in hook configuration SHALL set `async: true` so the user's session is not blocked.

#### Scenario: the benchmark is run as part of acceptance

- **WHEN** the documented benchmark is executed
- **THEN** the p95 latency is recorded
- **AND** the recorded value is referenced in the change's validation notes

### Requirement: `kenn cc-hook install` produces the hook-config snippet

The kenn CLI SHALL expose `cc-hook install` that prints, to stdout, the exact JSON snippet to add to `~/.claude/settings.json` to wire `SessionStart`, `UserPromptSubmit`, `PostToolUse` (matcher `Edit|Write|Read`), and `SessionEnd` to the corresponding `kenn cc-hook` subsubcommands. The subcommand SHALL accept an optional `--write` flag that merges the snippet into the user's `~/.claude/settings.json` in place; without `--write` it SHALL only print and not modify the settings file.

#### Scenario: install prints by default

- **WHEN** `kenn cc-hook install` is invoked without flags
- **THEN** the snippet is printed to stdout
- **AND** `~/.claude/settings.json` is not modified

#### Scenario: install --write modifies settings

- **WHEN** `kenn cc-hook install --write` is invoked
- **THEN** the snippet is merged into `~/.claude/settings.json`
- **AND** the resulting file remains valid JSON
- **AND** previously-configured hooks for other events are preserved
