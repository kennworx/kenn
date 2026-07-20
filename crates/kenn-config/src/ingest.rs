//! `[ingest]` section — sink batching knobs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestConfig {
    /// Records per sink batch. Smaller = lower memory, more transactions;
    /// larger = fewer transactions, higher memory.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
        }
    }
}

// Mirrors `kenn_store::api::DEFAULT_BATCH_SIZE`. Duplicated here so
// `kenn-config` doesn't have to pull in the store crate (and risk a
// circular dependency through the config-consuming crates).
const fn default_batch_size() -> usize {
    10_000
}
