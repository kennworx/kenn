//! Per-user kenn configuration: server lifecycle settings and
//! embedding-provider selection.
//!
//! Loaded from the OS-standard config path (`~/.config/kenn/kenn.toml`
//! on Linux/XDG, `~/Library/Application Support/kenn/kenn.toml` on
//! macOS, `%APPDATA%\kenn\kenn.toml` on Windows) via the `directories`
//! crate.
//!
//! Precedence on every field: env var > global config file > built-in
//! default.
//!
//! - `KENN_SERVER_ADDR` overrides `[server].addr`.
//! - `KENN_EMBED_URL` overrides `[embeddings].url`.
//! - `KENN_EMBED_MODEL` overrides `[embeddings].model`.
//! - `KENN_EMBED_BATCH_SIZE` overrides `[embeddings].batch_size` (the producer's
//!   per-request cap *and* the embed pass's scan-chunk size).

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub embeddings: EmbeddingsConfig,
}

/// `[server]` section — the kenn server's bind address.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// HTTP listen address. Default: `127.0.0.1:41873`. Override:
    /// `KENN_SERVER_ADDR`.
    #[serde(default = "default_server_addr")]
    pub addr: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: default_server_addr(),
        }
    }
}

fn default_server_addr() -> String {
    "127.0.0.1:41873".to_owned()
}

/// `[embeddings]` section — embedding-provider selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingsConfig {
    /// Provider base URL. When set, the client sends embed requests to
    /// this endpoint and SHALL NOT auto-spawn a kenn server. When
    /// unset, the selector probes / auto-spawns the local kenn server.
    /// Override: `KENN_EMBED_URL`.
    #[serde(default)]
    pub url: Option<String>,
    /// Model id sent in `/v1/embeddings` and written to the sidecar
    /// manifest. Default: `embeddinggemma-300M`. Override:
    /// `KENN_EMBED_MODEL`.
    #[serde(default = "default_embed_model")]
    pub model: String,
    /// Maximum number of inputs per `POST /v1/embeddings` request from
    /// the remote producer. A single `embed(texts)` call with more than
    /// this many inputs is split into multiple HTTP requests transparently.
    /// Bounds per-request body size and per-request latency so a large
    /// bulk pass can't exceed the client's HTTP timeout. It is also the
    /// **embed pass's scan-chunk size**, so the pass never hands the producer
    /// more than the producer batches — the two layers disagreeing is what let
    /// a full pass hold an entire corpus of vectors in memory. Default: 256.
    /// Override: `KENN_EMBED_BATCH_SIZE`.
    #[serde(default = "default_embed_batch_size")]
    pub batch_size: usize,
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            url: None,
            model: default_embed_model(),
            batch_size: default_embed_batch_size(),
        }
    }
}

fn default_embed_model() -> String {
    "embeddinggemma-300M".to_owned()
}

const fn default_embed_batch_size() -> usize {
    256
}

impl GlobalConfig {
    /// Parse a `GlobalConfig` from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(s)?)
    }

    /// Load from `path`, returning defaults when the file is missing.
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, ConfigError> {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            Ok(toml::from_str(&text)?)
        } else {
            Ok(Self::default())
        }
    }

    /// The per-OS standard global-config path
    /// (`<config_dir>/kenn/kenn.toml`). Returns `None` when no config
    /// directory is resolvable for the current user (rare — happens on
    /// platforms where the `directories` crate can't determine a home).
    #[must_use]
    pub fn standard_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "kenn").map(|d| d.config_dir().join("kenn.toml"))
    }

    /// Load the global config from the per-OS standard path (or return
    /// defaults if absent or unresolvable), then apply env-var
    /// overrides.
    pub fn load() -> Result<Self, ConfigError> {
        let mut cfg = match Self::standard_path() {
            Some(p) => Self::load_or_default(&p)?,
            None => Self::default(),
        };
        cfg.apply_env()?;
        Ok(cfg)
    }

    /// Overlay env-var overrides onto an in-memory `GlobalConfig`.
    /// Returns an error only when an env value fails validation (e.g.
    /// `KENN_SERVER_ADDR` is not a valid socket address).
    pub fn apply_env(&mut self) -> Result<(), ConfigError> {
        if let Ok(addr) = std::env::var("KENN_SERVER_ADDR") {
            // Validate eagerly so the user sees a clear error at load
            // time rather than at bind time.
            addr.parse::<SocketAddr>().map_err(|e| ConfigError::Addr {
                source_name: "KENN_SERVER_ADDR",
                value: addr.clone(),
                error: e,
            })?;
            self.server.addr = addr;
        }
        if let Ok(url) = std::env::var("KENN_EMBED_URL") {
            if !url.is_empty() {
                self.embeddings.url = Some(url);
            }
        }
        if let Ok(model) = std::env::var("KENN_EMBED_MODEL") {
            if !model.is_empty() {
                self.embeddings.model = model;
            }
        }
        // Also the embed pass's scan-chunk size, which is deliberately the same
        // value — an override that moved only the producer's HTTP batching
        // would recreate the split this setting exists to prevent.
        if let Ok(n) = std::env::var("KENN_EMBED_BATCH_SIZE") {
            if let Ok(n) = n.parse::<usize>() {
                if n > 0 {
                    self.embeddings.batch_size = n;
                }
            }
        }
        Ok(())
    }

    /// The resolved bind address, parsed from the (possibly
    /// env-overridden) `[server].addr` string.
    pub fn server_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.server.addr.parse().map_err(|e| ConfigError::Addr {
            source_name: "[server].addr",
            value: self.server.addr.clone(),
            error: e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, GlobalConfig};
    use std::sync::Mutex;

    // Env-var tests must serialize: tests run in parallel by default,
    // and the process env is shared mutable state. One mutex guards
    // every test that reads or writes our env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that snapshots and restores `KENN_*` env vars so
    /// concurrent tests can't leak state into each other.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn capture(vars: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let saved = vars.iter().map(|&k| (k, std::env::var(k).ok())).collect();
            for &k in vars {
                std::env::remove_var(k);
            }
            Self { saved, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn defaults_when_empty_toml() {
        let c = GlobalConfig::from_toml("").unwrap();
        assert_eq!(c.server.addr, "127.0.0.1:41873");
        assert!(c.embeddings.url.is_none());
        assert_eq!(c.embeddings.model, "embeddinggemma-300M");
    }

    #[test]
    fn missing_file_yields_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("absent.toml");
        let c = GlobalConfig::load_or_default(&nonexistent).unwrap();
        assert_eq!(c.server.addr, "127.0.0.1:41873");
        assert_eq!(c.embeddings.model, "embeddinggemma-300M");
    }

    #[test]
    fn partial_toml_keeps_defaults_for_absent_fields() {
        // Override only the model; addr and url should keep their defaults.
        let c = GlobalConfig::from_toml(
            r#"
[embeddings]
model = "custom-model"
"#,
        )
        .unwrap();
        assert_eq!(c.server.addr, "127.0.0.1:41873");
        assert!(c.embeddings.url.is_none());
        assert_eq!(c.embeddings.model, "custom-model");
    }

    #[test]
    fn full_toml_round_trips() {
        let c = GlobalConfig::from_toml(
            r#"
[server]
addr = "127.0.0.1:50000"

[embeddings]
url = "http://localhost:11434"
model = "nomic-embed-text"
"#,
        )
        .unwrap();
        assert_eq!(c.server.addr, "127.0.0.1:50000");
        assert_eq!(c.embeddings.url.as_deref(), Some("http://localhost:11434"));
        assert_eq!(c.embeddings.model, "nomic-embed-text");
    }

    #[test]
    fn env_overrides_beat_config_values() {
        let _g = EnvGuard::capture(&["KENN_SERVER_ADDR", "KENN_EMBED_URL", "KENN_EMBED_MODEL"]);
        std::env::set_var("KENN_SERVER_ADDR", "127.0.0.1:60000");
        std::env::set_var("KENN_EMBED_URL", "http://from-env:9999");
        std::env::set_var("KENN_EMBED_MODEL", "from-env-model");

        let mut c = GlobalConfig::from_toml(
            r#"
[server]
addr = "127.0.0.1:50000"

[embeddings]
url = "http://from-config:11434"
model = "from-config-model"
"#,
        )
        .unwrap();
        c.apply_env().unwrap();
        assert_eq!(c.server.addr, "127.0.0.1:60000");
        assert_eq!(c.embeddings.url.as_deref(), Some("http://from-env:9999"));
        assert_eq!(c.embeddings.model, "from-env-model");
    }

    #[test]
    fn env_unset_leaves_config_unchanged() {
        let _g = EnvGuard::capture(&["KENN_SERVER_ADDR", "KENN_EMBED_URL", "KENN_EMBED_MODEL"]);
        let mut c = GlobalConfig::from_toml(
            r#"
[server]
addr = "127.0.0.1:50000"

[embeddings]
model = "from-config-model"
"#,
        )
        .unwrap();
        c.apply_env().unwrap();
        assert_eq!(c.server.addr, "127.0.0.1:50000");
        assert!(c.embeddings.url.is_none());
        assert_eq!(c.embeddings.model, "from-config-model");
    }

    #[test]
    fn empty_env_value_is_ignored_for_optional_fields() {
        // Unsetting via `unset` is the standard way to disable an env
        // override; a literal empty string is treated as unset for the
        // optional fields so they keep their config/default value.
        let _g = EnvGuard::capture(&["KENN_EMBED_URL", "KENN_EMBED_MODEL"]);
        std::env::set_var("KENN_EMBED_URL", "");
        std::env::set_var("KENN_EMBED_MODEL", "");
        let mut c = GlobalConfig::from_toml(
            r#"
[embeddings]
url = "http://from-config:11434"
model = "from-config-model"
"#,
        )
        .unwrap();
        c.apply_env().unwrap();
        assert_eq!(
            c.embeddings.url.as_deref(),
            Some("http://from-config:11434")
        );
        assert_eq!(c.embeddings.model, "from-config-model");
    }

    #[test]
    fn bogus_addr_in_env_returns_clear_error() {
        let _g = EnvGuard::capture(&["KENN_SERVER_ADDR"]);
        std::env::set_var("KENN_SERVER_ADDR", "not-an-address");
        let mut c = GlobalConfig::default();
        let err = c.apply_env().expect_err("should fail");
        match err {
            ConfigError::Addr {
                source_name, value, ..
            } => {
                assert_eq!(source_name, "KENN_SERVER_ADDR");
                assert_eq!(value, "not-an-address");
            }
            other => panic!("expected ConfigError::Addr, got {other:?}"),
        }
    }

    #[test]
    fn server_addr_parses_resolved_value() {
        let c = GlobalConfig::default();
        let addr = c.server_addr().unwrap();
        assert_eq!(addr.port(), 41873);
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn server_addr_surfaces_parse_error_when_invalid() {
        let mut c = GlobalConfig::default();
        c.server.addr = "garbage".to_owned();
        let err = c.server_addr().expect_err("should fail");
        match err {
            ConfigError::Addr {
                source_name, value, ..
            } => {
                assert_eq!(source_name, "[server].addr");
                assert_eq!(value, "garbage");
            }
            other => panic!("expected ConfigError::Addr, got {other:?}"),
        }
    }
}
