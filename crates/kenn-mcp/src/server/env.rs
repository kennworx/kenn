/// True when `name` matches one of the host-relevant env-var prefixes
/// the `debug_env` tool surfaces. Lifted out so the prefix matrix is
/// independently testable without depending on process env state.
fn is_host_env(name: &str) -> bool {
    name.starts_with("CLAUDE_")
        || name == "CLAUDECODE"
        || name.starts_with("MCP_")
        || name == "AI_AGENT"
        || name.starts_with("XDG_")
        || name == "HOME"
}

/// Env snapshot for the `debug_env` tool. Filters to host-relevant
/// prefixes so the output is safe to paste in issue threads.
pub(super) fn debug_env_snapshot() -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let env: Map<String, Value> = std::env::vars()
        .filter(|(k, _)| is_host_env(k))
        .map(|(k, v)| (k, Value::String(v)))
        .collect();

    let cwd =
        std::env::current_dir().map_or(Value::Null, |p| Value::String(p.display().to_string()));

    json!({
        "pid": std::process::id(),
        "cwd": cwd,
        "env": Value::Object(env),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_env_filter_matches_known_prefixes() {
        // Positive matches — every name a host (Claude Code, Cursor,
        // Zed, generic XDG) might leak into an MCP subprocess.
        for name in [
            "CLAUDE_PROJECT_DIR",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_PLUGIN_ROOT",
            "CLAUDECODE",
            "MCP_TIMEOUT",
            "MCP_TOOL_TIMEOUT",
            "AI_AGENT",
            "XDG_CONFIG_HOME",
            "XDG_DATA_DIRS",
            "HOME",
        ] {
            assert!(is_host_env(name), "expected {name} to match");
        }

        // Negative matches — keep generic / credential-bearing vars
        // out of the dump so the JSON output is safe to share.
        for name in [
            "PATH",
            "LANG",
            "USER",
            "SHELL",
            "TERM",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "ANTHROPIC_API_KEY",
            "PWD",
            "OLDPWD",
            "TMPDIR",
        ] {
            assert!(!is_host_env(name), "did not expect {name} to match");
        }
    }

    #[test]
    fn debug_env_snapshot_includes_required_top_level_keys() {
        let v = debug_env_snapshot();
        let obj = v.as_object().expect("snapshot is a JSON object");
        for key in ["pid", "cwd", "env"] {
            assert!(obj.contains_key(key), "missing top-level key {key}");
        }
        assert!(obj["pid"].is_number(), "pid must be a number");
        assert!(obj["env"].is_object(), "env must be an object");
    }
}
