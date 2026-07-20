//! End-to-end smoke for `kenn cc-hook`.
//!
//! Each test spawns the binary in a child process with a realistic Claude Code
//! hook JSON on stdin and asserts the side effects land in the global
//! collector store. `$KENN_STATE_DIR` is pointed at a per-test tempdir so the
//! binary never touches the real user state dir.

use std::path::Path;

use assert_cmd::Command;
use rusqlite::Connection;
use tempfile::TempDir;

/// Spawn `kenn cc-hook <action>` with `stdin` as the payload and the collector
/// DB isolated under `state_dir`. `cwd` becomes the event's working dir.
fn cc_hook(state_dir: &Path, cwd: &Path, action: &str, stdin: &str) -> std::process::Output {
    Command::cargo_bin("kenn")
        .expect("locate kenn binary")
        .env("KENN_STATE_DIR", state_dir)
        .env("CLAUDE_PROJECT_DIR", cwd)
        .env_remove("USER")
        .args(["cc-hook", action])
        .write_stdin(stdin.to_owned())
        .output()
        .expect("spawn kenn cc-hook")
}

fn db(state_dir: &Path) -> Connection {
    Connection::open(state_dir.join("collector.db")).expect("open collector.db")
}

#[test]
fn end_to_end_session_capture_writes_rows() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let session = "smoke-1";

    let out = cc_hook(
        state.path(),
        ws.path(),
        "session-start",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}","source":"startup"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "session-start exit: {:?}", out.status);

    for p in ["first prompt", "second prompt"] {
        let out = cc_hook(
            state.path(),
            ws.path(),
            "prompt",
            &format!(
                r#"{{"session_id":"{session}","cwd":"{}","prompt":"{p}"}}"#,
                ws.path().display()
            ),
        );
        assert!(out.status.success(), "prompt exit: {:?}", out.status);
    }

    // A Bash command writing a log via tee — PreToolUse records it running.
    let out = cc_hook(
        state.path(),
        ws.path(),
        "pretool-bash",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}","tool_use_id":"tu-1","tool_name":"Bash","tool_input":{{"command":"cargo test 2>&1 | tee ./tmp/test.log"}}}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "pretool exit: {:?}", out.status);

    // An Edit touch — path-only file row.
    let out = cc_hook(
        state.path(),
        ws.path(),
        "touch",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}","tool_name":"Edit","tool_input":{{"file_path":"/abs/x.rs","old_string":"a","new_string":"b"}}}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "touch exit: {:?}", out.status);

    // PostToolUse finishes the Bash command.
    let out = cc_hook(
        state.path(),
        ws.path(),
        "posttool-bash",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}","tool_use_id":"tu-1","tool_name":"Bash","tool_response":{{"exit_code":0}}}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "posttool exit: {:?}", out.status);

    let out = cc_hook(
        state.path(),
        ws.path(),
        "session-end",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "session-end exit: {:?}", out.status);

    assert_lifecycle_rows(&db(state.path()), session, ws.path());
}

#[test]
fn session_start_injects_tee_instruction_on_stdout() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let out = cc_hook(
        state.path(),
        ws.path(),
        "session-start",
        &format!(
            r#"{{"session_id":"inj-1","cwd":"{}","source":"startup"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "session-start exit: {:?}", out.status);

    // Stdout carries the Claude Code additionalContext block naming tee; nothing
    // else is written there (stderr is the sink for diagnostics).
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    assert!(
        stdout.contains(r#""hookEventName":"SessionStart""#),
        "expected a SessionStart additionalContext block: {stdout}"
    );
    assert!(
        stdout.contains("tee"),
        "instruction should mention tee: {stdout}"
    );
    assert!(
        stdout.contains("squeeze"),
        "instruction should mention the squeeze-before-commit reminder: {stdout}"
    );
}

#[test]
fn session_start_resume_does_not_reinject() {
    // `resume` replays history that already holds the original injection, so we
    // skip it. Capture still happens; only stdout stays empty.
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let out = cc_hook(
        state.path(),
        ws.path(),
        "session-start",
        &format!(
            r#"{{"session_id":"inj-2","cwd":"{}","source":"resume"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "session-start exit: {:?}", out.status);
    assert!(
        out.stdout.is_empty(),
        "resume should not inject: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Assert the rows the lifecycle test expects: session (prompt + `ended_at`),
/// command (finished + exit 0), tee file row (absolutized), Edit touch row
/// (path-only, NULL `command_id`).
fn assert_lifecycle_rows(conn: &Connection, session: &str, ws: &Path) {
    let (last_prompt, ended): (String, Option<i64>) = conn
        .query_row(
            "SELECT last_prompt, ended_at FROM sessions WHERE id = ?1",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("session row");
    assert_eq!(last_prompt, "second prompt");
    assert!(ended.is_some(), "session-end sets ended_at");

    let (finished, exit): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT finished_at, exit_code FROM commands WHERE tool_use_id = 'tu-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("command row");
    assert!(finished.is_some(), "posttool sets finished_at");
    assert_eq!(exit, Some(0));

    let tee_path = ws.join("tmp").join("test.log");
    let tee_channel: String = conn
        .query_row(
            "SELECT channel FROM files WHERE path = ?1",
            [tee_path.to_string_lossy().as_ref()],
            |r| r.get(0),
        )
        .expect("tee file row");
    assert_eq!(tee_channel, "tee");

    let (edit_channel, edit_cmd): (String, Option<i64>) = conn
        .query_row(
            "SELECT channel, command_id FROM files WHERE path = '/abs/x.rs'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("edit file row");
    assert_eq!(edit_channel, "edit");
    assert!(edit_cmd.is_none(), "edit touch has NULL command_id");
}

#[test]
fn session_start_captures_tmux_and_provenance() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let out = Command::cargo_bin("kenn")
        .unwrap()
        .env("KENN_STATE_DIR", state.path())
        .env("CLAUDE_PROJECT_DIR", ws.path())
        .env("USER", "ada")
        .env("TMUX_PANE", "%5")
        .env("TMUX", "/tmp/tmux-1000/default,12345,3")
        .args(["cc-hook", "session-start"])
        .write_stdin(format!(
            r#"{{"session_id":"sm","cwd":"{}","source":"resume","transcript_path":"/t/conv.jsonl"}}"#,
            ws.path().display()
        ))
        .output()
        .unwrap();
    assert!(out.status.success(), "exit: {:?}", out.status);

    let conn = db(state.path());
    let get = |col: &str| -> Option<String> {
        conn.query_row(
            &format!("SELECT {col} FROM sessions WHERE id = 'sm'"),
            [],
            |r| r.get(0),
        )
        .expect("session row")
    };
    assert_eq!(get("source").as_deref(), Some("resume"));
    assert_eq!(get("transcript_path").as_deref(), Some("/t/conv.jsonl"));
    assert_eq!(get("os_user").as_deref(), Some("ada"));
    assert_eq!(get("tmux_pane").as_deref(), Some("%5"));
    // The tmux socket is the field of $TMUX before the first comma.
    assert_eq!(
        get("tmux_socket").as_deref(),
        Some("/tmp/tmux-1000/default")
    );
}

#[test]
fn read_tool_is_not_captured() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let out = cc_hook(
        state.path(),
        ws.path(),
        "touch",
        &format!(
            r#"{{"session_id":"r","cwd":"{}","tool_name":"Read","tool_input":{{"file_path":"/abs/x.rs"}}}}"#,
            ws.path().display()
        ),
    );
    // Exits 0 (graceful), but writes no file row for Read.
    assert!(out.status.success(), "exit: {:?}", out.status);
    let conn = db(state.path());
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/abs/x.rs'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(n, 0, "Read should not produce a files row");
}

#[test]
fn malformed_payload_exits_zero_and_logs() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let out = cc_hook(state.path(), ws.path(), "prompt", "not json");
    assert!(
        out.status.success(),
        "exit was {:?}, expected 0",
        out.status
    );
    // Diagnostic log present under the state dir.
    let log = state.path().join("cc-hook.log");
    assert!(log.is_file(), "cc-hook.log not written");
    let body = std::fs::read_to_string(&log).unwrap();
    assert!(body.contains("prompt bad json"), "log body: {body}");
}

#[test]
fn install_prints_snippet_without_write() {
    let out = Command::cargo_bin("kenn")
        .unwrap()
        .args(["cc-hook", "install"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("printed valid JSON");
    let hooks = parsed["hooks"].as_object().unwrap();
    for ev in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "SessionEnd",
        "Notification",
        "Stop",
        "SubagentStop",
    ] {
        assert!(hooks.contains_key(ev), "missing {ev} in snippet");
    }
}

#[test]
fn install_write_merges_into_settings_file() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    std::fs::write(
        &settings,
        r#"{"theme":"dark","hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"other"}]}]}}"#,
    )
    .unwrap();

    let out = Command::cargo_bin("kenn")
        .unwrap()
        .args(["cc-hook", "install", "--write"])
        .arg("--settings")
        .arg(&settings)
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let body = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["theme"], "dark", "pre-existing key preserved");
    // PostToolUse: pre-existing `other` + our two (posttool-bash, touch).
    let post = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 3, "pre-existing + ours, got {post:?}");
}

#[test]
fn prompt_then_stop_transitions_working_to_idle() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let session = "status-1";

    let out = cc_hook(
        state.path(),
        ws.path(),
        "prompt",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}","prompt":"go"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "prompt exit: {:?}", out.status);
    let status: String = db(state.path())
        .query_row(
            "SELECT status FROM sessions WHERE id = ?1",
            [session],
            |r| r.get(0),
        )
        .expect("status row");
    assert_eq!(status, "working", "prompt → working");

    let out = cc_hook(
        state.path(),
        ws.path(),
        "stop",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "stop exit: {:?}", out.status);
    let status: String = db(state.path())
        .query_row(
            "SELECT status FROM sessions WHERE id = ?1",
            [session],
            |r| r.get(0),
        )
        .expect("status row");
    assert_eq!(status, "idle", "stop → idle");
}

#[test]
fn notification_with_permission_message_is_needs_permission() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let session = "status-2";
    let message = "Claude needs your permission to use Bash";

    let out = cc_hook(
        state.path(),
        ws.path(),
        "notification",
        &format!(
            r#"{{"session_id":"{session}","cwd":"{}","message":"{message}"}}"#,
            ws.path().display()
        ),
    );
    assert!(out.status.success(), "notification exit: {:?}", out.status);

    let status: String = db(state.path())
        .query_row(
            "SELECT status FROM sessions WHERE id = ?1",
            [session],
            |r| r.get(0),
        )
        .expect("status row");
    assert_eq!(status, "needs_permission");

    // The raw message is preserved as the transition detail.
    let stored_detail: String = db(state.path())
        .query_row(
            "SELECT detail FROM session_status WHERE session_id = ?1",
            [session],
            |r| r.get(0),
        )
        .expect("session_status row");
    assert_eq!(stored_detail, message, "raw message stored as detail");
}

#[test]
fn pretool_task_then_subagent_stop_tracks_active_count() {
    let state = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let session = "status-3";
    let payload = format!(
        r#"{{"session_id":"{session}","cwd":"{}"}}"#,
        ws.path().display()
    );

    for _ in 0..2 {
        let out = cc_hook(state.path(), ws.path(), "pretool-task", &payload);
        assert!(out.status.success(), "pretool-task exit: {:?}", out.status);
    }
    let out = cc_hook(state.path(), ws.path(), "subagent-stop", &payload);
    assert!(out.status.success(), "subagent-stop exit: {:?}", out.status);

    let active: i64 = db(state.path())
        .query_row(
            "SELECT active_subagents FROM sessions WHERE id = ?1",
            [session],
            |r| r.get(0),
        )
        .expect("session row");
    assert_eq!(active, 1, "+1 +1 -1 = 1 active subagent");
}
