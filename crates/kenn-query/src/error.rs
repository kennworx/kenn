//! Query error model — `mcp-server` design D14.
//!
//! Distinguishes recoverable conditions (returned in the response envelope)
//! from unrecoverable ones. Each error carries a stable `code` string and an
//! optional `data` payload that the agent uses to retry.
//!
//! The codes are facts about the QUERY — a cursor outliving its snapshot, a
//! model still loading — and both front ends render them. Mapping them onto
//! JSON-RPC's numeric space is a convention of one wire format, and lives with
//! that transport in `server/errors.rs`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QueryErrorCode {
    StaleCursor,
    IndexUnavailable,
    /// Embedder backend selection is still running (cold start or
    /// reselection after the previous remote became unreachable).
    /// Agent should retry the same call shortly. See
    /// `kenn_embed::EmbedError::Starting`.
    EmbedderStarting,
    /// Snapshot has zero symbols — either no `kenn.toml` exists at all
    /// (`not-initialized`, suggest `kenn init`), no language is enabled
    /// in an existing `kenn.toml` (`config-disabled`), or every enabled
    /// language found nothing to index (`configured-but-empty`). Agent
    /// should consult `data.kind` + `data.enabled_languages` to choose
    /// between initializing, updating config, and inspecting the
    /// workspace contents.
    EmptySnapshot,
    /// `find_similar` was asked for a symbol with no committed embedding —
    /// the vectors are not built (run `kenn embed`) or this symbol has no
    /// embeddable text. Distinct from an empty-but-built result (no neighbours).
    EmbeddingUnavailable,
    InvalidInput,
    InternalError,
}

impl QueryErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StaleCursor => "STALE_CURSOR",
            Self::IndexUnavailable => "INDEX_UNAVAILABLE",
            Self::EmbedderStarting => "EMBEDDER_STARTING",
            Self::EmptySnapshot => "EMPTY_SNAPSHOT",
            Self::EmbeddingUnavailable => "EMBEDDING_UNAVAILABLE",
            Self::InvalidInput => "INVALID_INPUT",
            Self::InternalError => "INTERNAL_ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryError {
    pub code: QueryErrorCode,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl QueryError {
    #[must_use]
    pub fn new(code: QueryErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    #[must_use]
    pub fn stale_cursor(expected: &str, current: &str) -> Self {
        Self::new(
            QueryErrorCode::StaleCursor,
            "Index was rebuilt during pagination",
        )
        .with_data(serde_json::json!({
            "expected_snapshot_id": expected,
            "current_snapshot_id": current,
        }))
    }

    #[must_use]
    pub fn index_unavailable() -> Self {
        Self::new(
            QueryErrorCode::IndexUnavailable,
            "No `.kenn/live` snapshot. Run `kenn index` first.",
        )
    }

    /// Tool was called while the MCP server is in `Indexing` state.
    /// Agent should poll `get_index_status` and retry once `state`
    /// becomes `"ready"`.
    #[must_use]
    pub fn index_unavailable_indexing() -> Self {
        Self::new(
            QueryErrorCode::IndexUnavailable,
            "indexing in progress; call `wait_for_index` to block until it is ready (or `get_index_status` to observe state), then retry",
        )
    }

    /// Tool was called while the MCP server is in `Failed` state.
    /// Carries the pipeline error (in both the message and `data.reason`)
    /// **plus an explicit recovery path** — without it agents tend to
    /// invent a cause ("looks like a path/permission issue") from the
    /// raw store error and abandon kenn instead of retrying. The `reindex`
    /// tool transitions `Failed → Indexing` and re-runs the pipeline.
    #[must_use]
    pub fn index_unavailable_failed(reason: &str) -> Self {
        Self::new(
            QueryErrorCode::IndexUnavailable,
            format!(
                "indexing failed: {reason}. To recover: the file watcher retries on the \
                 next file change, or call the `reindex` tool to retry now; if it persists, \
                 run `kenn index` to rebuild the index."
            ),
        )
        .with_data(serde_json::json!({ "reason": reason }))
    }

    /// Embedder backend selection is still running. Agent should retry
    /// the same call shortly (selection completes in ms after a warm
    /// daemon probe, up to ~5 s on a cold daemon spawn). The internal
    /// `reason` (which state-machine arm produced this) is dropped from
    /// the wire message — the agent retry contract is the same
    /// regardless — but the server logs it for debugging.
    #[must_use]
    pub fn embedder_starting(reason: &str) -> Self {
        tracing::debug!(target: "kenn_mcp::embedder", reason, "surfacing EMBEDDER_STARTING to agent");
        Self::new(
            QueryErrorCode::EmbedderStarting,
            "embedder warming up; retry shortly",
        )
    }

    /// Data-returning MCP tool was called against a snapshot with zero
    /// symbols. The classification distinguishes "no `kenn.toml` at all"
    /// (not-initialized, suggest `kenn init`), "no language enabled in
    /// `kenn.toml`" (config-disabled), and "configured language found
    /// nothing to index" (configured-but-empty); all share the
    /// `EMPTY_SNAPSHOT` string code and the `-32002` service-unavailable
    /// JSON-RPC code.
    #[must_use]
    pub fn empty_snapshot(hint: &ConfigHint) -> Self {
        // The recovery prose is computed once on the hint (`ConfigHint::new`);
        // the error message and the overview's `config_hint.suggestion` are
        // the same string. `kenn_subcode = "EMPTY_SNAPSHOT"` is added by the
        // wire layer (server.rs::into_error_data); agents branch on
        // `data.kenn_subcode` for the code and `data.{kind,enabled_languages}`
        // for the classifier.
        Self::new(QueryErrorCode::EmptySnapshot, hint.suggestion.clone()).with_data(
            serde_json::json!({
                "kind": hint.kind.as_str(),
                "enabled_languages": hint.enabled_languages,
            }),
        )
    }
}

/// Classification of an empty snapshot, surfaced both via
/// [`QueryError::empty_snapshot`] (on data-returning tools) and the
/// `config_hint` field of `get_workspace_overview` (always succeeds).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigHint {
    pub kind: ConfigHintKind,
    pub enabled_languages: Vec<String>,
    /// Human-readable recovery action — the same prose the
    /// `EMPTY_SNAPSHOT` error carries, embedded here so the orientation
    /// call (`get_workspace_overview`) delivers the next step (e.g. "run
    /// `kenn init`") rather than a bare `kind` enum the agent has to
    /// interpret. Without it agents read `not-initialized` as "index not
    /// built" and fall back to manual exploration.
    pub suggestion: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigHintKind {
    /// No `kenn.toml` in the workspace — kenn was never set up here.
    /// The agent should suggest running `kenn init`.
    NotInitialized,
    ConfigDisabled,
    ConfiguredButEmpty,
}

impl ConfigHintKind {
    /// Stable wire string, mirrored in the `data.kind` error payload.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInitialized => "not-initialized",
            Self::ConfigDisabled => "config-disabled",
            Self::ConfiguredButEmpty => "configured-but-empty",
        }
    }
}

impl ConfigHint {
    /// Build a hint with its recovery `suggestion` derived from `kind`
    /// (and the enabled-language list, for the configured-but-empty
    /// case). The single source of truth for the prose — both the
    /// overview hint and the `EMPTY_SNAPSHOT` error message read it.
    #[must_use]
    pub fn new(kind: ConfigHintKind, enabled_languages: Vec<String>) -> Self {
        let suggestion = match kind {
            ConfigHintKind::NotInitialized => {
                "kenn is not set up for this project — no `kenn.toml` found. \
                 Run `kenn init` to create it, enable your language(s) under \
                 `[language.*]`, then run `kenn index`."
                    .to_string()
            }
            ConfigHintKind::ConfigDisabled => {
                "Snapshot is empty — no languages enabled in `kenn.toml`. \
                 Set `[language.csharp]`, `[language.rust]`, \
                 `[language.typescript]`, or `[language.python]` to \
                 `enabled = true`, then run `kenn index` to populate the index."
                    .to_string()
            }
            ConfigHintKind::ConfiguredButEmpty => {
                let langs = enabled_languages
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "Snapshot is empty — enabled language(s) ({langs}) produced no \
                     symbols. Check `kenn status` / `report.json` for indexer \
                     failures, or confirm the workspace contains source files \
                     outside excluded directories."
                )
            }
        };
        Self {
            kind,
            enabled_languages,
            suggestion,
        }
    }

    /// Classify a snapshot's symbol count against the workspace's
    /// language config. Returns `None` when the snapshot has symbols
    /// (no hint applies); `Some(NotInitialized)` when no `kenn.toml`
    /// exists in the workspace (`config_present == false`) — kenn was
    /// never set up here; `Some(ConfigDisabled)` when `kenn.toml` exists
    /// but every `[language.*].enabled` is false; `Some(ConfiguredButEmpty)`
    /// when at least one language is enabled but no symbols were produced.
    /// Language identifiers in `enabled_languages` use the canonical
    /// lowercase serialization (`csharp`, `rust`, `typescript`,
    /// `python`).
    #[must_use]
    pub fn classify(
        config: &kenn_config::Config,
        symbol_count: u64,
        config_present: bool,
    ) -> Option<Self> {
        if symbol_count > 0 {
            return None;
        }
        if !config_present {
            return Some(Self::new(ConfigHintKind::NotInitialized, Vec::new()));
        }
        let mut enabled_languages = Vec::new();
        if config.language.csharp.enabled {
            enabled_languages.push("csharp".to_string());
        }
        if config.language.rust.enabled {
            enabled_languages.push("rust".to_string());
        }
        if config.language.typescript.enabled {
            enabled_languages.push("typescript".to_string());
        }
        if config.language.python.enabled {
            enabled_languages.push("python".to_string());
        }
        if config.language.go.enabled {
            enabled_languages.push("go".to_string());
        }
        if config.language.swift.enabled {
            enabled_languages.push("swift".to_string());
        }
        let kind = if enabled_languages.is_empty() {
            ConfigHintKind::ConfigDisabled
        } else {
            ConfigHintKind::ConfiguredButEmpty
        };
        Some(Self::new(kind, enabled_languages))
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for QueryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_strings_stable() {
        assert_eq!(QueryErrorCode::StaleCursor.as_str(), "STALE_CURSOR");
        assert_eq!(
            QueryErrorCode::IndexUnavailable.as_str(),
            "INDEX_UNAVAILABLE"
        );
        assert_eq!(QueryErrorCode::EmptySnapshot.as_str(), "EMPTY_SNAPSHOT");
        assert_eq!(QueryErrorCode::InvalidInput.as_str(), "INVALID_INPUT");
        assert_eq!(QueryErrorCode::InternalError.as_str(), "INTERNAL_ERROR");
    }

    #[test]
    fn config_hint_returns_none_for_healthy_snapshot() {
        let cfg = kenn_config::Config::default();
        // Symbol count > 0 short-circuits before the config-present check.
        assert!(ConfigHint::classify(&cfg, 1, true).is_none());
        assert!(ConfigHint::classify(&cfg, 1_000_000, false).is_none());
    }

    #[test]
    fn config_hint_returns_not_initialized_when_no_config_present() {
        // No `kenn.toml` on disk — kenn was never set up here. Wins over
        // the config-disabled classification even though the default
        // config has every language off.
        let cfg = kenn_config::Config::default();
        let hint = ConfigHint::classify(&cfg, 0, false).expect("empty snapshot must yield a hint");
        assert_eq!(hint.kind, ConfigHintKind::NotInitialized);
        assert!(hint.enabled_languages.is_empty());
        // The hint itself carries the action, so `get_workspace_overview`
        // (which returns the struct, not an error) delivers the next step
        // and not just a bare `kind` enum...
        assert!(
            hint.suggestion.contains("kenn init"),
            "suggestion: {}",
            hint.suggestion
        );
        // ...and the EMPTY_SNAPSHOT error message is the same prose.
        let err = QueryError::empty_snapshot(&hint);
        assert_eq!(err.message, hint.suggestion);
    }

    #[test]
    fn config_hint_returns_config_disabled_when_all_languages_off() {
        // Default Config has every language disabled (post-uniform-config-shape).
        let cfg = kenn_config::Config::default();
        let hint = ConfigHint::classify(&cfg, 0, true).expect("empty snapshot must yield a hint");
        assert_eq!(hint.kind, ConfigHintKind::ConfigDisabled);
        assert!(hint.enabled_languages.is_empty());
    }

    #[test]
    fn config_hint_returns_configured_but_empty_when_python_enabled() {
        let mut cfg = kenn_config::Config::default();
        cfg.language.python.enabled = true;
        let hint = ConfigHint::classify(&cfg, 0, true).expect("empty snapshot must yield a hint");
        assert_eq!(hint.kind, ConfigHintKind::ConfiguredButEmpty);
        assert_eq!(hint.enabled_languages, vec!["python".to_string()]);
    }

    #[test]
    fn empty_snapshot_error_carries_full_data_payload() {
        let hint = ConfigHint::new(
            ConfigHintKind::ConfiguredButEmpty,
            vec!["python".to_string()],
        );
        let err = QueryError::empty_snapshot(&hint);
        assert_eq!(err.code, QueryErrorCode::EmptySnapshot);
        // `kenn_subcode = "EMPTY_SNAPSHOT"` is injected by the wire layer
        // — QueryError::data carries only the classifier payload.
        let data = err.data.expect("empty_snapshot error must carry data");
        assert_eq!(data["kind"], "configured-but-empty");
        assert_eq!(data["enabled_languages"][0], "python");
        assert!(err.message.contains("python"));
    }

    #[test]
    fn empty_snapshot_message_lists_four_language_keys_when_config_disabled() {
        let hint = ConfigHint::new(ConfigHintKind::ConfigDisabled, Vec::new());
        let err = QueryError::empty_snapshot(&hint);
        assert!(err.message.contains("kenn.toml"));
        for lang in ["csharp", "rust", "typescript", "python"] {
            assert!(
                err.message.contains(lang),
                "message missing language key `{lang}`: {}",
                err.message
            );
        }
    }

    #[test]
    fn index_unavailable_failed_carries_reason_and_recovery_path() {
        let err = QueryError::index_unavailable_failed("opening store: disk I/O error");
        assert_eq!(err.code, QueryErrorCode::IndexUnavailable);
        // The raw reason survives for both human reading and structured
        // consumers (`data.reason`)...
        assert!(err.message.contains("opening store: disk I/O error"));
        assert_eq!(err.data.unwrap()["reason"], "opening store: disk I/O error");
        // ...and the message names a concrete recovery action so the agent
        // retries instead of guessing at the cause.
        assert!(
            err.message.contains("reindex") && err.message.contains("kenn index"),
            "failed-state message must point at a recovery path: {}",
            err.message
        );
    }

    #[test]
    fn stale_cursor_carries_both_ids() {
        let e = QueryError::stale_cursor("aabbccddeeff", "112233445566");
        assert_eq!(e.code, QueryErrorCode::StaleCursor);
        let d = e.data.unwrap();
        assert_eq!(d["expected_snapshot_id"], "aabbccddeeff");
        assert_eq!(d["current_snapshot_id"], "112233445566");
    }
}
