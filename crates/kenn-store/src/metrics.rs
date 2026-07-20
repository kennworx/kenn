//! Quality-metric report on flip — `index-lifecycle` §Quality-metric report.
//!
//! After every successful flip the store compares aggregated counters from
//! the new run against the previous snapshot's recorded counters. Drops
//! exceeding `threshold_pct` per metric become [`RegressionWarning`]s, which
//! are persisted in the new run's report and surfaced via `kenn
//! status`. Warnings never block the flip.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub documents: u64,
    pub symbols: u64,
    pub definitions: u64,
    pub edges: u64,
    pub failed_projects: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegressionWarning {
    pub metric: &'static str,
    pub previous: u64,
    pub current: u64,
    /// Percentage drop, rounded down. `(prev - cur) * 100 / prev`.
    pub drop_pct: u32,
}

#[must_use]
pub fn compute_diff(
    prev: &MetricSnapshot,
    new: &MetricSnapshot,
    threshold_pct: u32,
) -> Vec<RegressionWarning> {
    let mut out = Vec::new();
    for (name, prev_v, new_v) in [
        ("documents", prev.documents, new.documents),
        ("symbols", prev.symbols, new.symbols),
        ("definitions", prev.definitions, new.definitions),
        ("edges", prev.edges, new.edges),
    ] {
        if let Some(w) = regression(name, prev_v, new_v, threshold_pct) {
            out.push(w);
        }
    }
    out
}

fn regression(
    metric: &'static str,
    previous: u64,
    current: u64,
    threshold_pct: u32,
) -> Option<RegressionWarning> {
    if previous == 0 || current >= previous {
        return None;
    }
    let drop = previous - current;
    // Use u128 to avoid overflow on very large counters.
    let drop_pct =
        u32::try_from((u128::from(drop) * 100) / u128::from(previous)).unwrap_or(u32::MAX);
    if drop_pct < threshold_pct {
        return None;
    }
    Some(RegressionWarning {
        metric,
        previous,
        current,
        drop_pct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(documents: u64, symbols: u64, definitions: u64, edges: u64) -> MetricSnapshot {
        MetricSnapshot {
            documents,
            symbols,
            definitions,
            edges,
            failed_projects: 0,
        }
    }

    #[test]
    fn thirty_percent_document_drop_warns() {
        let prev = snap(100, 1000, 500, 2000);
        let new = snap(70, 1000, 500, 2000);
        let warns = compute_diff(&prev, &new, 10);
        assert_eq!(warns.len(), 1);
        assert_eq!(warns[0].metric, "documents");
        assert_eq!(warns[0].drop_pct, 30);
    }

    #[test]
    fn five_percent_drift_silent() {
        let prev = snap(100, 1000, 500, 2000);
        let new = snap(95, 950, 475, 1900);
        assert!(compute_diff(&prev, &new, 10).is_empty());
    }

    #[test]
    fn growth_is_not_a_regression() {
        let prev = snap(100, 1000, 500, 2000);
        let new = snap(200, 2000, 1000, 4000);
        assert!(compute_diff(&prev, &new, 10).is_empty());
    }

    #[test]
    fn empty_previous_silent() {
        // First-ever flip: prev counters all zero. No regressions to report.
        let prev = snap(0, 0, 0, 0);
        let new = snap(100, 1000, 500, 2000);
        assert!(compute_diff(&prev, &new, 10).is_empty());
    }

    #[test]
    fn multiple_metrics_dropping() {
        let prev = snap(100, 1000, 500, 2000);
        let new = snap(50, 400, 500, 2000);
        let warns = compute_diff(&prev, &new, 10);
        let metrics: Vec<_> = warns.iter().map(|w| w.metric).collect();
        assert_eq!(metrics, vec!["documents", "symbols"]);
    }
}
