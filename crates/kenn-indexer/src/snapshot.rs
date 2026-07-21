//! Snapshot metadata + run-artifact persistence, shared by the CLI
//! (`kenn index`) and the workflow / MCP `index_workspace` path.
//!
//! Single source of truth for a run's on-disk artifacts — `meta.json`,
//! `report.json`, `overview.md` — so both entry paths persist an identical,
//! HONEST snapshot (real aggregate status, failed projects, source root) rather
//! than drifting. `kenn status` reads [`SnapshotMeta`] back.

use std::path::Path;

use serde::{Deserialize, Serialize};

use kenn_store::staleness::StalenessKey;

use crate::report::{
    collect_failed_overflow, collect_failed_projects, collect_warnings, collect_warnings_overflow,
    render_overview_md, render_with_overflow, RunReport, RunStatus, ToolchainVersion,
};

/// The run-local metadata file, written into every published snapshot.
pub const SNAPSHOT_META_FILE: &str = "meta.json";

/// Aggregated pipeline counters persisted to `meta.json`.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct SnapshotCounts {
    pub documents: u64,
    pub symbols: u64,
    pub definitions: u64,
    pub edges: u64,
}

/// Sum the per-unit counters across every report.
#[must_use]
pub fn aggregate_counts(reports: &[RunReport]) -> SnapshotCounts {
    let mut c = SnapshotCounts::default();
    for r in reports {
        c.documents = c.documents.saturating_add(r.files_seen);
        c.symbols = c.symbols.saturating_add(r.symbols_seen);
        c.definitions = c.definitions.saturating_add(r.defs_seen);
        c.edges = c.edges.saturating_add(r.edges_seen);
    }
    c
}

/// Persisted alongside each snapshot at publish time. Keeps `kenn status` and
/// the staleness check independent of the indexer's per-unit `RunReport`s —
/// those are language-driver specific.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub timestamp: String,
    pub status: String,
    /// Active kenn-store backend at index time (`"sqlite"`). `None` for
    /// snapshots written before the marker existed.
    #[serde(default)]
    pub backend: Option<String>,
    /// Store schema version recorded at publish time. `None` resolves to `1`.
    #[serde(default)]
    pub schema_version: Option<u32>,
    /// Absolute path of the indexed source root; `None` for older snapshots.
    #[serde(default)]
    pub source_root: Option<String>,
    pub documents: u64,
    pub symbols: u64,
    pub definitions: u64,
    pub edges: u64,
    #[serde(default)]
    pub failed_projects: Vec<String>,
    /// Attributions dropped past per-unit retention caps — the true failure
    /// count is `failed_projects.len() + failed_overflow`. Display surfaces
    /// render the overflow as a `+N more` suffix.
    #[serde(default)]
    pub failed_overflow: u64,
    /// Status-neutral producer diagnostics (bounded like `failed_projects`)
    /// — e.g. stale index-store units kept on a trusted read. Shown by
    /// `kenn status`.
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub warnings_overflow: u64,
    #[serde(default)]
    pub regression_warnings: Vec<RegressionWarning>,
    /// Toolchains the entrypoint provisioned for this run, deduplicated across
    /// units (language + resolved version). Diagnostic provenance, not a
    /// staleness input. `None`/empty for older snapshots and toolchain-free
    /// producers.
    #[serde(default)]
    pub toolchains: Vec<ToolchainVersion>,
    #[serde(default)]
    pub staleness_key: Option<serde_json::Value>,
}

impl SnapshotMeta {
    /// Read and parse `<dir>/meta.json`. `None` when the file is absent
    /// (a pre-reporting snapshot) — the single read home for both
    /// `kenn status` and the MCP status surface. A file that is present
    /// but unparsable is logged at `warn` and also yields `None`, so a
    /// schema drift is diagnosable rather than a silent empty report.
    #[must_use]
    pub fn read(dir: &Path) -> Option<Self> {
        let path = dir.join(SNAPSHOT_META_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(
                    target: "kenn_indexer::snapshot",
                    path = %path.display(), error = %e,
                    "reading snapshot meta.json"
                );
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(meta) => Some(meta),
            Err(e) => {
                tracing::warn!(
                    target: "kenn_indexer::snapshot",
                    path = %path.display(), error = %e,
                    "parsing snapshot meta.json; degraded-run report unavailable for this snapshot"
                );
                None
            }
        }
    }

    /// True number of failed projects — the bounded list plus attributions
    /// dropped past the per-unit retention cap. The `+N more` marker is a
    /// display concern; counting consumers use this.
    #[must_use]
    pub fn failed_total(&self) -> u64 {
        self.failed_projects.len() as u64 + self.failed_overflow
    }

    /// True number of status-neutral warnings (bounded list + overflow).
    #[must_use]
    pub fn warning_total(&self) -> u64 {
        self.warnings.len() as u64 + self.warnings_overflow
    }

    /// Number of metric regressions recorded against the prior snapshot.
    #[must_use]
    pub fn regression_total(&self) -> u64 {
        self.regression_warnings.len() as u64
    }

    /// A run that produced **nothing to report** — no failed projects, no
    /// warnings, no metric regressions. Status-independent by
    /// construction (a clean run is exactly one with no diagnostics), so
    /// it does not couple to the stringly-typed `status` field.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed_total() == 0 && self.warning_total() == 0 && self.regression_warnings.is_empty()
    }
}

/// A metric-regression warning recorded in `meta.json` (the CLI computes these
/// against the previous snapshot; the workflow/MCP path records none).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionWarning {
    pub metric: String,
    pub previous: u64,
    pub current: u64,
    pub drop_pct: u32,
}

/// Build the snapshot metadata from a run's aggregate `status`, `counts`,
/// `reports`, and `regressions` (empty on the workflow/MCP path).
#[must_use]
pub fn build_snapshot_meta(
    run_id: &str,
    status: RunStatus,
    counts: &SnapshotCounts,
    reports: &[RunReport],
    regressions: Vec<kenn_store::RegressionWarning>,
    staleness: &StalenessKey,
    source_root: &Path,
) -> SnapshotMeta {
    SnapshotMeta {
        timestamp: run_id.to_string(),
        status: format!("{status:?}").to_lowercase(),
        backend: Some(kenn_store::ACTIVE_BACKEND.to_string()),
        schema_version: Some(kenn_store::STORE_SCHEMA_VERSION),
        source_root: Some(source_root.display().to_string()),
        documents: counts.documents,
        symbols: counts.symbols,
        definitions: counts.definitions,
        edges: counts.edges,
        failed_projects: collect_failed_projects(reports),
        failed_overflow: collect_failed_overflow(reports),
        warnings: collect_warnings(reports),
        warnings_overflow: collect_warnings_overflow(reports),
        regression_warnings: regressions
            .into_iter()
            .map(|w| RegressionWarning {
                metric: w.metric.to_string(),
                previous: w.previous,
                current: w.current,
                drop_pct: w.drop_pct,
            })
            .collect(),
        toolchains: collect_toolchains(reports),
        // `to_value` on a plain serializable key does not realistically fail;
        // fall back to `None` rather than propagate an error just for that.
        staleness_key: serde_json::to_value(staleness).ok(),
    }
}

/// Union of every unit's provisioned toolchains, deduplicated on
/// (language, version) and ordered by language then version, so the run
/// summary lists each provisioned toolchain once regardless of how many units
/// shared it.
fn collect_toolchains(reports: &[RunReport]) -> Vec<ToolchainVersion> {
    let mut seen: Vec<ToolchainVersion> = Vec::new();
    for report in reports {
        for tc in &report.toolchains {
            if !seen.contains(tc) {
                seen.push(tc.clone());
            }
        }
    }
    seen.sort_by(|a, b| a.language.cmp(&b.language).then(a.version.cmp(&b.version)));
    seen
}

/// Write the run's three artifacts into `run_dir`: `meta.json` (lifecycle
/// completion stamp), `report.json` (per-unit diagnostic), and `overview.md`
/// (agent orientation — its absence is the "not built yet" signal).
pub fn persist_run_artifacts(
    run_dir: &Path,
    meta: &SnapshotMeta,
    reports: &[RunReport],
) -> std::io::Result<()> {
    std::fs::write(
        run_dir.join(SNAPSHOT_META_FILE),
        serde_json::to_vec_pretty(meta)?,
    )?;
    std::fs::write(
        run_dir.join("report.json"),
        serde_json::to_vec_pretty(reports)?,
    )?;
    std::fs::write(
        run_dir.join("overview.md"),
        render_overview_md(
            &meta.timestamp,
            &meta.status,
            meta.documents,
            meta.symbols,
            meta.definitions,
            meta.edges,
            meta.source_root.as_deref().unwrap_or("unknown"),
            &render_with_overflow(&meta.failed_projects, meta.failed_overflow),
            &meta.toolchains,
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_with(toolchains: &[(&str, &str)]) -> RunReport {
        let mut r = RunReport::started("csharp", "0.1.0", "unit");
        r.toolchains = toolchains
            .iter()
            .map(|(l, v)| ToolchainVersion {
                language: (*l).into(),
                version: (*v).into(),
            })
            .collect();
        r
    }

    /// The run summary lists each provisioned toolchain ONCE, ordered by
    /// language, even when several units shared it and reported it out of order.
    #[test]
    fn collect_toolchains_dedups_and_sorts() {
        let reports = [
            report_with(&[("go", "1.24.5")]),
            report_with(&[("dotnet", "9.0.308"), ("go", "1.24.5")]),
        ];
        let got = collect_toolchains(&reports);
        assert_eq!(
            got,
            vec![
                ToolchainVersion {
                    language: "dotnet".into(),
                    version: "9.0.308".into()
                },
                ToolchainVersion {
                    language: "go".into(),
                    version: "1.24.5".into()
                },
            ]
        );
    }
}
