//! `[mcp]` section — MCP-server-specific knobs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// Start the in-process file watcher at server boot. The watcher is
    /// the primary freshness driver (watcher-driven-staleness), so this
    /// defaults to `true`; an explicit `watch_stop` tool call still
    /// disables it for the session.
    #[serde(default = "default_watch_on")]
    pub watch_on: bool,
    /// Debounce window for the file watcher: collapse edit bursts
    /// within this many milliseconds of inactivity into a single
    /// reindex trigger.
    #[serde(default = "default_watch_debounce_ms")]
    pub watch_debounce_ms: u64,
    /// Cadence (seconds) of the low-frequency git staleness backstop —
    /// it re-checks the staleness key on `spawn_blocking` to catch OS
    /// watcher events that were dropped, and (after `watch_stop`) is the
    /// only freshness mechanism left. `0` disables it. Default 300s.
    #[serde(default = "default_staleness_backstop_secs")]
    pub staleness_backstop_secs: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            watch_on: default_watch_on(),
            watch_debounce_ms: default_watch_debounce_ms(),
            staleness_backstop_secs: default_staleness_backstop_secs(),
        }
    }
}

const fn default_watch_on() -> bool {
    true
}

const fn default_watch_debounce_ms() -> u64 {
    30_000
}

const fn default_staleness_backstop_secs() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn defaults() {
        let c = Config::from_toml("").unwrap();
        // Watcher is the primary freshness driver, so it defaults on.
        assert!(c.mcp.watch_on);
        assert_eq!(c.mcp.watch_debounce_ms, 30_000);
        assert_eq!(c.mcp.staleness_backstop_secs, 300);
    }

    #[test]
    fn parses_overrides() {
        let toml = r"
[mcp]
watch_on = true
watch_debounce_ms = 5000
";
        let c = Config::from_toml(toml).unwrap();
        assert!(c.mcp.watch_on);
        assert_eq!(c.mcp.watch_debounce_ms, 5000);
    }
}
