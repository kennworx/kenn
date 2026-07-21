use super::*;
use serde_json::json;
use tempfile::TempDir;

/// Channel-matching the `touch` handler uses, factored for tests. The DB
/// round-trips (redirect/tee capture, Pre→Post running state, edit/write
/// path-only rows, concurrency) are covered by `kenn-collect`'s store and
/// parser tests, which can read the connection directly.
fn touch_channel(payload: &[u8]) -> Result<FileChannel, String> {
    let input = decode(payload, "touch")?;
    match input.tool_name.as_deref().unwrap_or("") {
        "Edit" => Ok(FileChannel::Edit),
        "Write" => Ok(FileChannel::Write),
        other => Err(format!("touch for unexpected tool: {other}")),
    }
}

#[test]
fn touch_accepts_edit_and_write() {
    let edit = br#"{"session_id":"s","tool_name":"Edit","tool_input":{"file_path":"/a"}}"#;
    let write = br#"{"session_id":"s","tool_name":"Write","tool_input":{"file_path":"/a"}}"#;
    assert_eq!(touch_channel(edit).unwrap(), FileChannel::Edit);
    assert_eq!(touch_channel(write).unwrap(), FileChannel::Write);
}

#[test]
fn read_tool_is_rejected_by_touch() {
    // D7: the touch matcher narrowed from Edit|Write|Read to Edit|Write.
    // Read is an error (logged, exit 0) and writes no row.
    let read = br#"{"session_id":"s","tool_name":"Read","tool_input":{"file_path":"/a"}}"#;
    let err = touch_channel(read).unwrap_err();
    assert!(err.contains("Read"), "Read should be rejected: {err}");
}

#[test]
fn extract_exit_code_variants() {
    assert_eq!(extract_exit_code(Some(&json!({"exit_code": 2}))), 2);
    assert_eq!(extract_exit_code(Some(&json!({"exitCode": 7}))), 7);
    assert_eq!(extract_exit_code(Some(&json!({"interrupted": true}))), 130);
    assert_eq!(extract_exit_code(Some(&json!({"is_error": true}))), 1);
    assert_eq!(extract_exit_code(Some(&json!({"stdout": "hi"}))), 0);
    assert_eq!(extract_exit_code(None), 0);
}

#[test]
fn decode_rejects_malformed_json() {
    let _ = decode(b"not json", "prompt").unwrap_err();
    let _ = decode(b"   ", "prompt").unwrap_err();
}

#[test]
fn resolve_cwd_prefers_payload_cwd() {
    let input = HookInput {
        cwd: Some("/explicit".to_string()),
        ..Default::default()
    };
    assert_eq!(resolve_cwd(&input), "/explicit");
}

#[test]
fn install_snippet_has_all_events_and_narrowed_touch_matcher() {
    let s = install::snippet();
    let hooks = s.get("hooks").and_then(Value::as_object).unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "SessionEnd",
        "Notification",
        "Stop",
        "SubagentStop",
    ] {
        assert!(hooks.contains_key(event), "missing {event}");
    }
    // PostToolUse has two matchers: Bash and Edit|Write (not Read).
    let post = hooks["PostToolUse"].as_array().unwrap();
    let matchers: Vec<&str> = post.iter().filter_map(|e| e["matcher"].as_str()).collect();
    assert!(matchers.contains(&"Bash"));
    assert!(matchers.contains(&"Edit|Write"));
    assert!(!matchers.iter().any(|m| m.contains("Read")));
    // PreToolUse has two matchers: Bash and Task.
    let pre = hooks["PreToolUse"].as_array().unwrap();
    let pre_matchers: Vec<&str> = pre.iter().filter_map(|e| e["matcher"].as_str()).collect();
    assert!(pre_matchers.contains(&"Bash"));
    assert!(pre_matchers.contains(&"Task"));
}

#[test]
fn install_merge_creates_new_settings() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.json");
    install::merge_into(&path, &install::snippet()).unwrap();
    let parsed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let hooks = parsed["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 8);
}

#[test]
fn install_merge_preserves_unrelated_hooks_and_settings() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.json");
    let existing = json!({
        "theme": "dark",
        "hooks": {
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "other-tool log-bash"}]
            }]
        }
    });
    std::fs::write(&path, serde_json::to_string(&existing).unwrap()).unwrap();
    install::merge_into(&path, &install::snippet()).unwrap();
    let parsed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(parsed["theme"], "dark");
    // Other-tool's PostToolUse entry preserved alongside our two.
    let post = parsed["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 3, "expected pre-existing + ours, got {post:?}");
}

#[test]
fn install_merge_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.json");
    install::merge_into(&path, &install::snippet()).unwrap();
    install::merge_into(&path, &install::snippet()).unwrap();
    let parsed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (event, want) in [
        ("SessionStart", 1),
        ("UserPromptSubmit", 1),
        ("PreToolUse", 2),
        ("PostToolUse", 2),
        ("SessionEnd", 1),
        ("Notification", 1),
        ("Stop", 1),
        ("SubagentStop", 1),
    ] {
        let arr = parsed["hooks"][event].as_array().unwrap();
        assert_eq!(arr.len(), want, "{event} should not duplicate, got {arr:?}");
    }
}
