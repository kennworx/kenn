//! `[staleness]` section — when to skip a redundant indexer pass.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StalenessConfig {
    /// Compare `(HEAD, dirty xxhashes)` against the live snapshot's
    /// recorded key and skip the run on match.
    #[serde(default = "default_true")]
    pub git_aware_skip: bool,
}

impl Default for StalenessConfig {
    fn default() -> Self {
        Self {
            git_aware_skip: true,
        }
    }
}

const fn default_true() -> bool {
    true
}
