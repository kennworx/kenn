//! `[metrics]` section — regression thresholds.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    /// Per-metric drop percentage that triggers a regression warning.
    #[serde(default = "default_regression_threshold")]
    pub regression_threshold_pct: u32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            regression_threshold_pct: default_regression_threshold(),
        }
    }
}

const fn default_regression_threshold() -> u32 {
    10
}
