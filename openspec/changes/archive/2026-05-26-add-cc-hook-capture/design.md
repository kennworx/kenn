## Context

Claude Code emits hook events synchronously around user prompts, tool calls, and session lifecycle. Hooks receive JSON on stdin including `session_id`, `transcript_path`, `cwd`, `tool_name`, `tool_input`, and for `PostToolUse` the `result`. The `Edit`/`Write` tools expose `file_path` plus `old_string`/`new_string` directly. `Bash` is opaque; file mutations driven through shell are not attributable.

Kenn already has a store layout that splits `<store>/` into a committed subtree (`findings/`, `vectors/`) and a derived subtree (`local/`, gitignored as a whole). This change adds `local/history/` and `local/history/ready/` slots under the derived subtree — inheriting the existing gitignore without per-feature rules — plus a CLI subcommand to feed them. There is no ingest, no embedding, no LLM, and no read surface in this change — those are deliberate follow-ups whose design has been sketched (see the exploration notes in commit history and Open Questions below) but is intentionally not committed until we have real captured data to look at.

The full feature was explored end-to-end before scoping down — the decisions for ingest, verdict classification, session topics, MCP tools, and the system-prompt fragment are well-formed and will land as follow-up changes. The risk this slicing manages is committing to a Lance schema and an LLM prompt design before the raw shape of real captures has been observed.

## Goals / Non-Goals

**Goals:**
- Capture every Claude Code session that opts in, with zero perceptible latency added to interactive use.
- Make the raw data inspectable with standard tools (`jq`, `tail`) so we can study the corpus before committing to a processing pipeline.
- Reserve the store layout slots that follow-up changes will use, so the next change is purely additive (no migrations).

**Non-Goals:**
- Any processing of the raw events. No embedding, no Lance, no LLM, no MCP, no UI.
- Multi-user sharing tests. The layout makes sharing trivial later; v1 doesn't validate it.
- Bash-driven file mutations. Same accepted gap as the full design.

## Decisions

### D1: Hook layer is a dumb append

Hooks invoke `kenn cc-hook <event>`, which reads the hook JSON on stdin and appends one record to `local/history/<session_id>.jsonl`. No embedding, no LLM, no Lance write, no network. Target latency ≤5ms p95.

*Alternative considered:* hook writes directly to a structured store. Rejected for the same reasons it was rejected in the full design — keeps the hook trivial, lets the (future) ingest batch.

### D2: Raw record shape is forward-compatible

Records are tagged-union JSON (`kind: "session_start" | "prompt" | "touch" | "session_end"`) with all the fields a future ingest will need: for `touch`, `file_path` + `old_string` + `new_string` + `tool_name`; for `prompt`, the prompt text; for `session_start` and `session_end`, the session-level metadata (`user`, `branch`, `git_sha`, `cwd`, `t_start`/`t_end`). Prompt↔touch grouping is recovered at ingest time from the monotonic `t` ordering rather than carried denormalized at the hook layer (see spec note). The schema is documented in the spec; future ingest changes consume it without requiring a v1 migration.

*Alternative considered:* store only the rawest possible hook payload. Rejected — we already know what fields ingest will want; flattening at capture is cheap and removes a parse step later.

### D3: SessionEnd writes a ready marker

`session-end` writes both the raw record and a zero-byte `local/history/ready/<session_id>` marker. The marker is unused in v1 but lets follow-up ingest cheaply enumerate finished sessions without scanning every JSONL file. Costs ~one inode per session.

*Alternative considered:* skip the marker until ingest lands. Rejected — adding it now costs nothing and keeps the v1→v2 transition purely additive on the ingest side.

### D4: Graceful failure, always exit 0

`kenn cc-hook` never returns non-zero on a recoverable error (malformed JSON, disk full, missing store). Internal errors get logged to a kenn diagnostic file but the hook exits 0 so the user's Claude Code session is never interrupted by capture failures. This is non-negotiable; capture must be invisible.

*Alternative considered:* exit non-zero on bad JSON to surface bugs. Rejected — bugs will surface via diagnostic logs and benchmark monitoring; user-visible failures during a working session are worse than silent loss.

### D5: Install helper is a separate subsubcommand

`kenn cc-hook install` prints (and optionally writes to `~/.claude/settings.json`) the exact JSON snippet wiring the four hook events to `kenn cc-hook ...`. Reduces the opt-in cost to one command and avoids users hand-editing settings.

*Alternative considered:* document the snippet in README only. Rejected — hand-editing settings is error-prone and discourages adoption.

## Risks / Trade-offs

- **Hook overhead on every Edit** → Mitigation: hook is Rust, stdin-driven, target ≤5ms p95. Benchmark before shipping; if it exceeds 10ms, switch hook config to `async: true` per the Claude Code hooks API.

- **Storage growth without retention policy** → ~1-5 KB per record, ~50 records per session, ~10 sessions/day per user ≈ 0.5-2.5 MB/day. Bounded and small. v1 ships with no retention policy; revisit if it crosses 1 GB.

- **Secrets in prompts get stored** → same trust boundary as findings. Document; do not scrub.

- **Parallel sessions** → Each session writes its own `<session_id>.jsonl` file, so cross-session contention is moot. Within a single session, the writer process is single-threaded (one Claude session = one stream of hook calls), so intra-file append ordering is also fine. Confirm during implementation.

- **Forward-compat risk** → The raw record schema commits us to certain field names. Mitigation: schema is documented in the spec; the tagged-union shape with explicit `kind` lets follow-up changes add new kinds without breaking old readers, and lets old readers skip unknown kinds.

- **Session never finishes (Claude crash)** → Raw JSONL exists, no ready marker. v1 doesn't care (no ingest). Follow-up ingest will need a stale-threshold sweep; the issue is documented but not solved here.

## Open Questions

- **Hook config matchers**: should `PostToolUse` match `Edit|Write|Read`, or `Edit|Write` only? Reads might be high-volume noise. Lean toward including Read so the raw corpus has it — we can decide later at ingest time whether to drop them, but we can't recover what we never captured.
- **Where does the kenn diagnostic log live?** Likely a fixed path under the kenn store. Pin during implementation.
- **`kenn cc-hook install` — write directly, or print and ask?** Lean toward print-by-default with a `--write` flag, to avoid surprising users who didn't expect their settings file to be touched.
