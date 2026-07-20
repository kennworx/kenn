//! `[lifecycle]` section — snapshot retention.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleConfig {
    /// Number of snapshots retained by GC. 2 = current + previous (rollback target).
    #[serde(default = "default_gc_keep")]
    pub gc_keep: usize,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            gc_keep: default_gc_keep(),
        }
    }
}

const fn default_gc_keep() -> usize {
    2
}
