//! `[language.*] runtime` — where a language's indexer launcher runs.

use serde::{Deserialize, Serialize};

/// Where a language's indexer `command` runs. `Local` (the default) spawns it on
/// the host `PATH`; `Docker` runs it inside the language's `image` via
/// `docker run` (see the `docker-indexer-runtime` change). Serialized in
/// `kenn.toml` as `runtime = "local"` / `runtime = "docker"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    #[default]
    Local,
    Docker,
}
