use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::Deserialize;
use serde_json::Value;

use kenn_collect::{AgentStatus, FileChannel, SessionMeta, Store};

use crate::exit::ExitCodes;

#[derive(Debug, Subcommand)]
pub enum CcHookAction {
    /// Print the hook-config snippet for `~/.claude/settings.json`.
    /// Without `--write` the snippet is printed to stdout only.
    Install {
        /// Merge the snippet into `~/.claude/settings.json` in place.
        #[arg(long)]
        write: bool,
        /// Override the settings path (testing only).
        #[arg(long, hide = true)]
        settings: Option<PathBuf>,
    },
    /// Capture a `SessionStart` hook payload from stdin.
    SessionStart,
    /// Capture a `UserPromptSubmit` hook payload from stdin.
    Prompt,
    /// Capture a `PreToolUse` (Bash) hook payload from stdin: record the
    /// command as running plus its parsed output files.
    PretoolBash,
    /// Capture a `PostToolUse` (Bash) hook payload from stdin: finish the
    /// matching command by `tool_use_id`.
    PosttoolBash,
    /// Capture a `PostToolUse` hook payload from stdin (matched for
    /// `Edit|Write`): record a path-only file row.
    Touch,
    /// Capture a `SessionEnd` hook payload from stdin.
    SessionEnd,
    /// Capture a `Stop` hook payload from stdin: the turn ended, so the
    /// session's agent status becomes `idle`.
    Stop,
    /// Capture a `SubagentStop` hook payload from stdin: a subagent ended, so
    /// the session's active-subagent count is decremented.
    SubagentStop,
    /// Capture a `Notification` hook payload from stdin: classify the message
    /// into `needs_permission` / `needs_input` and stamp the agent status.
    Notification,
    /// Capture a `PreToolUse` (Task) hook payload from stdin: a subagent was
    /// spawned, so the session's active-subagent count is incremented.
    PretoolTask,
}

// ── incoming hook payload ──────────────────────────────────────────

/// Subset of the Claude Code hook JSON we care about. Unknown fields are
/// ignored; missing optional fields default to `None` / empty. Claude Code
/// sends the linking id as `tool_use_id`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct HookInput {
    session_id: String,
    cwd: Option<String>,
    source: Option<String>,
    transcript_path: Option<String>,
    tool_use_id: Option<String>,
    prompt: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<Value>,
    tool_response: Option<Value>,
    message: Option<String>,
}

// ── entry point ────────────────────────────────────────────────────

/// Top-level entry from `main.rs`. The capture subcommands open the global
/// collector store and write directly; they always return `ExitCodes::Ok`.
/// `Install` may return a real error code since it is not in the hot path.
pub fn run_standalone(action: CcHookAction) -> ExitCodes {
    if let CcHookAction::Install { write, settings } = action {
        return install::run(write, settings.as_deref());
    }

    let mut stdin = Vec::with_capacity(8192);
    if let Err(e) = io::stdin().read_to_end(&mut stdin) {
        log_diag(&format!("stdin: {e}"));
        return ExitCodes::Ok;
    }

    let result = match action {
        CcHookAction::SessionStart => handle_session_start(&stdin),
        CcHookAction::Prompt => handle_prompt(&stdin),
        CcHookAction::PretoolBash => handle_pretool_bash(&stdin),
        CcHookAction::PosttoolBash => handle_posttool_bash(&stdin),
        CcHookAction::Touch => handle_touch(&stdin),
        CcHookAction::SessionEnd => handle_session_end(&stdin),
        CcHookAction::Stop => handle_stop(&stdin),
        CcHookAction::SubagentStop => handle_subagent_stop(&stdin),
        CcHookAction::Notification => handle_notification(&stdin),
        CcHookAction::PretoolTask => handle_pretool_task(&stdin),
        #[expect(
            clippy::unreachable,
            reason = "Install is dispatched before this match"
        )]
        CcHookAction::Install { .. } => unreachable!(),
    };
    if let Err(e) = result {
        log_diag(&e);
    }
    ExitCodes::Ok
}

// ── per-event handlers ─────────────────────────────────────────────

fn handle_session_start(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "session-start")?;
    // Inject the standing "use tee" instruction. Skip `resume` — the original
    // injection is already in the replayed history; `startup`/`clear`/`compact`
    // all begin from empty or trimmed context and need it (re)seeded. Runs
    // before the capture below, so the guidance lands even if the DB write
    // fails; emission errors go through tracing, never stdout.
    if input.source.as_deref() != Some("resume") {
        emit_session_start_instruction();
    }
    let cwd = resolve_cwd(&input);
    let meta = SessionMeta {
        source: input.source.clone(),
        transcript_path: input.transcript_path.clone(),
        os_user: std::env::var("USER").ok(),
        tmux_pane: std::env::var("TMUX_PANE").ok(),
        // `$TMUX` is `<socket-path>,<server-pid>,<session-id>`; keep the socket.
        tmux_socket: std::env::var("TMUX")
            .ok()
            .and_then(|t| t.split(',').next().map(str::to_owned)),
    };
    let mut store = open_store()?;
    store
        .start_session(&input.session_id, &cwd, &meta, now_secs())
        .map_err(|e| format!("session-start: {e}"))?;
    // Lazy GC piggybacks on session start (design §D9). Failure is
    // non-fatal — the next session will retry.
    drop(store.maybe_lazy_gc(now_secs()));
    Ok(())
}

fn handle_prompt(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "prompt")?;
    let Some(prompt) = input.prompt.as_deref() else {
        return Err("prompt payload has no prompt".to_string());
    };
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    // The session row may not exist yet (e.g. SessionStart was missed);
    // ensure it before setting the prompt.
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("prompt ensure-session: {e}"))?;
    store
        .set_last_prompt(&input.session_id, prompt, now_secs())
        .map_err(|e| format!("prompt set: {e}"))?;
    // A turn has started: the main agent is working (design D1).
    store
        .set_status(&input.session_id, AgentStatus::Working, None, now_secs())
        .map_err(|e| format!("prompt set_status: {e}"))
}

fn handle_pretool_bash(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "pretool-bash")?;
    let cwd = resolve_cwd(&input);
    let cmd = input
        .tool_input
        .as_ref()
        .and_then(|v| v.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("");

    // Parse — never fatal. On error we still record the command with no files.
    let parsed = kenn_collect::parse(cmd, Path::new(&cwd)).ok();
    let signature = parsed.as_ref().and_then(|p| p.signature.clone());
    let outputs = parsed.map(|p| p.outputs).unwrap_or_default();

    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("pretool ensure-session: {e}"))?;
    let command_id = store
        .insert_command(
            &input.session_id,
            input.tool_use_id.as_deref(),
            cmd,
            signature.as_deref(),
            &cwd,
            now_secs(),
        )
        .map_err(|e| format!("pretool insert_command: {e}"))?;
    for o in &outputs {
        store
            .insert_file(
                &input.session_id,
                Some(command_id),
                &cwd,
                &o.path,
                o.kind.into(),
                o.op.as_deref(),
                o.resolved,
                now_secs(),
            )
            .map_err(|e| format!("pretool insert_file: {e}"))?;
    }
    Ok(())
}

fn handle_posttool_bash(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "posttool-bash")?;
    let Some(tool_use_id) = input.tool_use_id.as_deref() else {
        return Err("posttool-bash payload has no tool_use_id".to_string());
    };
    let exit_code = extract_exit_code(input.tool_response.as_ref());
    let store = open_store()?;
    store
        .finish_command(tool_use_id, exit_code, now_secs())
        .map_err(|e| format!("posttool finish_command: {e}"))
}

fn handle_touch(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "touch")?;
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("touch ensure-session: {e}"))?;
    // D7: narrow to Edit|Write. Any other tool (e.g. Read) is rejected here
    // — the session row exists but no `files` row is written.
    let tool = input.tool_name.as_deref().unwrap_or("");
    let channel = match tool {
        "Edit" => FileChannel::Edit,
        "Write" => FileChannel::Write,
        other => return Err(format!("touch for unexpected tool: {other}")),
    };
    let file_path = input
        .tool_input
        .as_ref()
        .and_then(|v| v.get("file_path"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("touch tool_input has no file_path (tool={tool})"))?;
    // Edit/Write file_path is always absolute in Claude Code payloads;
    // `resolved` is true (we have a concrete path, no `command_id`).
    store
        .insert_file(
            &input.session_id,
            None,
            &cwd,
            file_path,
            channel,
            None,
            true,
            now_secs(),
        )
        .map_err(|e| format!("touch insert_file: {e}"))
}

fn handle_session_end(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "session-end")?;
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("session-end ensure-session: {e}"))?;
    store
        .end_session(&input.session_id, now_secs())
        .map_err(|e| format!("session-end: {e}"))
}

fn handle_stop(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "stop")?;
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("stop ensure-session: {e}"))?;
    // The turn ended: the main agent is idle (design D1).
    store
        .set_status(&input.session_id, AgentStatus::Idle, None, now_secs())
        .map_err(|e| format!("stop set_status: {e}"))
}

fn handle_subagent_stop(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "subagent-stop")?;
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("subagent-stop ensure-session: {e}"))?;
    // One in-flight subagent ended (design D2).
    store
        .bump_subagents(&input.session_id, -1, now_secs())
        .map_err(|e| format!("subagent-stop bump_subagents: {e}"))
}

fn handle_notification(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "notification")?;
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("notification ensure-session: {e}"))?;
    // Classify best-effort: a message naming a permission means the agent is
    // blocked on a permission prompt; otherwise it needs some other input
    // (design D1). The raw message is stored as the transition detail.
    let status = if input
        .message
        .as_deref()
        .is_some_and(|m| m.to_lowercase().contains("permission"))
    {
        AgentStatus::NeedsPermission
    } else {
        AgentStatus::NeedsInput
    };
    store
        .set_status(
            &input.session_id,
            status,
            input.message.as_deref(),
            now_secs(),
        )
        .map_err(|e| format!("notification set_status: {e}"))
}

fn handle_pretool_task(payload: &[u8]) -> Result<(), String> {
    let input = decode(payload, "pretool-task")?;
    let cwd = resolve_cwd(&input);
    let store = open_store()?;
    store
        .upsert_session(&input.session_id, &cwd, now_secs())
        .map_err(|e| format!("pretool-task ensure-session: {e}"))?;
    // A subagent was spawned (design D2).
    store
        .bump_subagents(&input.session_id, 1, now_secs())
        .map_err(|e| format!("pretool-task bump_subagents: {e}"))
}

// ── helpers ────────────────────────────────────────────────────────

fn decode(payload: &[u8], ctx: &str) -> Result<HookInput, String> {
    if payload.iter().all(u8::is_ascii_whitespace) {
        return Err(format!("{ctx}: empty payload"));
    }
    serde_json::from_slice(payload).map_err(|e| format!("{ctx} bad json: {e}"))
}

fn open_store() -> Result<Store, String> {
    Store::open().map_err(|e| format!("open store: {e}"))
}

/// Resolve the cwd for a hook event: payload `cwd` → `CLAUDE_PROJECT_DIR` →
/// git toplevel → process cwd. This is both the row's `cwd` (project is
/// derived from it) and the base for absolutizing parsed Bash outputs.
fn resolve_cwd(input: &HookInput) -> String {
    if let Some(c) = input.cwd.as_deref() {
        if !c.is_empty() {
            return c.to_string();
        }
    }
    if let Some(p) = std::env::var_os("CLAUDE_PROJECT_DIR") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return pb.to_string_lossy().into_owned();
        }
    }
    if let Some(p) = git_toplevel() {
        return p.to_string_lossy().into_owned();
    }
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn git_toplevel() -> Option<PathBuf> {
    kenn_store::git::work_dir(&std::env::current_dir().ok()?)
}

/// Pull an exit code out of the `PostToolUse` `tool_response` payload.
/// Accepts (in priority order): integer `exit_code`, integer `exitCode`,
/// `interrupted: true` → 130, `is_error`/`isError: true` → 1, default → 0.
fn extract_exit_code(resp: Option<&Value>) -> i32 {
    let Some(v) = resp else { return 0 };
    if let Some(n) = v.get("exit_code").and_then(Value::as_i64) {
        return i32::try_from(n).unwrap_or(0);
    }
    if let Some(n) = v.get("exitCode").and_then(Value::as_i64) {
        return i32::try_from(n).unwrap_or(0);
    }
    if v.get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return 130;
    }
    let err = v
        .get("is_error")
        .or_else(|| v.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    i32::from(err)
}

fn now_secs() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

// ── context injection ──────────────────────────────────────────────

/// Standing instructions injected into the agent's context at session start:
/// the `tee`-to-`./tmp` convention (so long-running output is captured and
/// tailable, and the redirect/tee targets land in the §D3 file rows) and the
/// advisory squeeze-before-commit reminder. Authored as markdown and embedded
/// at compile time.
const SESSION_START_INSTRUCTION: &str = include_str!("../../assets/session_start.md");

/// Emit the standing session instructions as a Claude Code `SessionStart`
/// `additionalContext` block on stdout — the hook's context-injection channel.
/// Stdout carries only this clean JSON; a serialize or write failure (e.g.
/// `BrokenPipe`) is logged via `tracing` and otherwise ignored, never surfaced
/// on stdout.
fn emit_session_start_instruction() {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": SESSION_START_INSTRUCTION.trim(),
        }
    });
    let line = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "cc-hook: serialize additionalContext");
            return;
        }
    };
    if let Err(e) = writeln!(io::stdout(), "{line}") {
        tracing::warn!(error = %e, "cc-hook: write additionalContext to stdout");
    }
}

// ── diagnostics ────────────────────────────────────────────────────

fn diag_log_path() -> Option<PathBuf> {
    // Co-locate the diagnostic log with the collector DB in the state dir, so
    // the `KENN_STATE_DIR` test override keeps both hermetic.
    kenn_collect::collector_state_dir()
        .ok()
        .map(|d| d.join("cc-hook.log"))
}

fn log_diag(msg: &str) {
    let Some(path) = diag_log_path() else { return };
    if let Some(parent) = path.parent() {
        drop(std::fs::create_dir_all(parent));
    }
    let line = format!("{} {msg}\n", now_secs());
    drop(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes())),
    );
}

// ── install helper ────────────────────────────────────────────────

mod install {
    use super::{ExitCodes, PathBuf, Value};
    use std::io::Write;

    /// The hook-config snippet that wires Claude Code's lifecycle + tool
    /// events to `kenn cc-hook ...`. Returned as a serde Value so the
    /// `--write` merge can compose it into existing settings.
    pub(super) fn snippet() -> Value {
        serde_json::json!({
            "hooks": {
                "SessionStart": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "kenn cc-hook session-start"}]
                }],
                "UserPromptSubmit": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "kenn cc-hook prompt"}]
                }],
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": "kenn cc-hook pretool-bash"}]
                    },
                    {
                        "matcher": "Task",
                        "hooks": [{"type": "command", "command": "kenn cc-hook pretool-task"}]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": "kenn cc-hook posttool-bash"}]
                    },
                    {
                        "matcher": "Edit|Write",
                        "hooks": [{"type": "command", "command": "kenn cc-hook touch"}]
                    }
                ],
                "SessionEnd": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "kenn cc-hook session-end"}]
                }],
                "Notification": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "kenn cc-hook notification"}]
                }],
                "Stop": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "kenn cc-hook stop"}]
                }],
                "SubagentStop": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "kenn cc-hook subagent-stop"}]
                }]
            }
        })
    }

    pub fn run(write: bool, settings_override: Option<&std::path::Path>) -> ExitCodes {
        let snippet = snippet();
        if !write {
            // Pretty-print so users can paste it directly. BrokenPipe
            // on stdout (e.g. piping to `jq`) is the standard "reader
            // done" signal — swallow it.
            let pretty =
                serde_json::to_string_pretty(&snippet).unwrap_or_else(|_| String::from("{}"));
            drop(std::io::stdout().write_all(pretty.as_bytes()));
            drop(std::io::stdout().write_all(b"\n"));
            return ExitCodes::Ok;
        }
        let path = match settings_override {
            Some(p) => p.to_path_buf(),
            None => default_settings_path().unwrap_or_else(|| PathBuf::from("settings.json")),
        };
        match merge_into(&path, &snippet) {
            Ok(()) => {
                eprintln!("kenn cc-hook: merged into {}", path.display());
                ExitCodes::Ok
            }
            Err(e) => {
                eprintln!("kenn cc-hook install --write failed: {e}");
                ExitCodes::Generic
            }
        }
    }

    fn default_settings_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".claude").join("settings.json"))
    }

    /// Merge our snippet into the user's settings file. Reads what's
    /// there (or starts with `{}` if absent), appends our entries to
    /// the matching event arrays, and writes back atomically. Skips
    /// entries whose `command` already matches one we'd add, so
    /// re-running is idempotent.
    pub(super) fn merge_into(path: &std::path::Path, snippet: &Value) -> std::io::Result<()> {
        let existing: Value = match std::fs::read_to_string(path) {
            Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("parse settings: {e}"),
                )
            })?,
            _ => serde_json::json!({}),
        };
        let mut merged = existing;
        let target_hooks = merged
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("settings root is not an object"))?
            .entry("hooks")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let target_hooks = target_hooks
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("settings.hooks is not an object"))?;

        let our_hooks = snippet
            .get("hooks")
            .and_then(Value::as_object)
            .expect("snippet always has object .hooks");

        for (event, our_entries_v) in our_hooks {
            let our_entries = our_entries_v
                .as_array()
                .expect("snippet entries are arrays");
            let dst = target_hooks
                .entry(event.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(dst_arr) = dst.as_array_mut() else {
                continue; // user has a non-array here; leave it alone
            };
            for entry in our_entries {
                if !contains_kenn_cc_hook(dst_arr, entry) {
                    dst_arr.push(entry.clone());
                }
            }
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&merged).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// True if `dst` already contains an entry whose nested
    /// `hooks[*].command` matches the candidate's command. Used for
    /// idempotency: rerunning `install --write` is a no-op.
    fn contains_kenn_cc_hook(dst: &[Value], candidate: &Value) -> bool {
        let Some(cand_cmd) = candidate
            .get("hooks")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|h| h.get("command"))
            .and_then(Value::as_str)
        else {
            return false;
        };
        dst.iter().any(|e| {
            e.get("hooks")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .any(|h| h.get("command").and_then(Value::as_str) == Some(cand_cmd))
        })
    }
}

// ── unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
