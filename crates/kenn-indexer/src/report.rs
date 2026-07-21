//! Per-run report. Captures totals, failed units, and producer-side
//! provenance counters required by tasks 5b.10 and 5c.5.

use serde::{Deserialize, Serialize};

/// Variant order is severity order (`Success < Partial < Failed`) — the
/// derived `Ord` is how consumers rank statuses (e.g. worst-of rollups),
/// so keep new variants in severity position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Success,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisionResult {
    Created,
    AlreadyExists,
    UserDeclined,
    NotRequested,
}

/// Provenance counter for FROM-attribution (task 5b.10): how each occurrence's
/// FROM symbol was determined.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnclosingRangeCounters {
    pub scip: u64,
    pub refinement: u64,
    pub heuristic: u64,
    pub dropped: u64,
}

/// Provenance counter for `SymbolKind` resolution (task 5c.5).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolKindCounters {
    pub scip: u64,
    pub descriptor: u64,
    pub unknown: u64,
}

/// A toolchain the provisioning entrypoint resolved for this run, from the
/// workspace's own pin file. Diagnostic provenance — which .NET SDK / Go / Rust
/// actually produced the index — so a result change is attributable rather than
/// silent. NOT a staleness input: the pin file is tracked, so the existing
/// staleness key already forces a reindex on a pin edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainVersion {
    pub language: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub indexer_name: String,
    /// Config language this unit indexed. Derived from `indexer_name` when
    /// it is a db language name; branded producers (`rust-analyzer`,
    /// `kenn-dotnet`, …) set it explicitly via [`RunReport::started_for`].
    /// `None` only for pre-existing persisted reports and auxiliary units
    /// with no single language (`html-resolve`, `<panicked>`).
    #[serde(default)]
    pub language: Option<kenn_model::Language>,
    pub indexer_version: String,
    pub unit: String,
    pub started_at: String,
    pub ended_at: String,
    pub files_seen: u64,
    pub symbols_seen: u64,
    #[serde(default)]
    pub defs_seen: u64,
    /// Definitions that carried an enclosing-item body extent (SCIP
    /// `enclosing_range`). For a Rust unit, `defs_seen > 0 && def_bodies_seen
    /// == 0` flags a too-old rust-analyzer that emits no `enclosing_range`.
    #[serde(default)]
    pub def_bodies_seen: u64,
    pub edges_seen: u64,
    /// Documents dropped because their canonicalized path fell outside the
    /// workspace root (`CanonicalizeError::OutsideRoot`) — a container/host
    /// root mismatch, a symlinked root, or a bad mount. Counted, not silently
    /// skipped: a run whose documents all landed here would otherwise publish
    /// an empty index at exit 0.
    #[serde(default)]
    pub out_of_root_seen: u64,
    #[serde(default)]
    pub failed_projects: Vec<String>,
    /// Failure attributions beyond the entries retained in
    /// `failed_projects` (bounded producers cap the list). Rendered as a
    /// `+N more` suffix at display time — kept structured so counting
    /// consumers never mistake a summary marker for a real failure.
    #[serde(default)]
    pub failed_overflow: u64,
    /// Warning-severity diagnostics from the producer's stream (bounded,
    /// like `failed_projects`). Status-neutral, but surfaced — producers
    /// emit these for degradations that keep the run useful (e.g. stale
    /// index-store units kept on a trusted read).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Warning attributions beyond the retained `warnings` entries.
    #[serde(default)]
    pub warnings_overflow: u64,
    pub status: RunStatus,
    #[serde(default)]
    pub enclosing_range_source: EnclosingRangeCounters,
    #[serde(default)]
    pub symbol_kind_source: SymbolKindCounters,
    #[serde(default)]
    pub directory_build_props: Option<ProvisionResult>,
    #[serde(default)]
    pub directory_build_props_hint: bool,
    /// Toolchains the entrypoint provisioned for this unit, from `toolchain`
    /// wire frames. Empty for producers with no external toolchain (markdown,
    /// css, html) and for runs against images that bundle their toolchain.
    #[serde(default)]
    pub toolchains: Vec<ToolchainVersion>,
}

impl RunReport {
    /// Start a report whose `indexer_name` is a db language name (or an
    /// auxiliary unit with no language) — `language` is derived from the
    /// name. Branded producers use [`RunReport::started_for`].
    #[must_use]
    pub fn started(indexer_name: &str, indexer_version: &str, unit: &str) -> Self {
        let now = current_iso8601();
        Self {
            indexer_name: indexer_name.into(),
            language: kenn_model::Language::from_db_name(indexer_name),
            indexer_version: indexer_version.into(),
            unit: unit.into(),
            started_at: now.clone(),
            ended_at: now,
            files_seen: 0,
            symbols_seen: 0,
            defs_seen: 0,
            def_bodies_seen: 0,
            edges_seen: 0,
            out_of_root_seen: 0,
            failed_projects: Vec::new(),
            failed_overflow: 0,
            warnings: Vec::new(),
            warnings_overflow: 0,
            status: RunStatus::Success,
            enclosing_range_source: EnclosingRangeCounters::default(),
            symbol_kind_source: SymbolKindCounters::default(),
            directory_build_props: None,
            directory_build_props_hint: false,
            toolchains: Vec::new(),
        }
    }

    /// Start a report for a branded producer (`rust-analyzer`,
    /// `kenn-dotnet`, …) whose `indexer_name` is not a db language name —
    /// the driver states its language explicitly so downstream grouping
    /// and labeling never reverse-engineer it from the brand.
    #[must_use]
    pub fn started_for(
        language: kenn_model::Language,
        indexer_name: &str,
        indexer_version: &str,
        unit: &str,
    ) -> Self {
        let mut r = Self::started(indexer_name, indexer_version, unit);
        r.language = Some(language);
        r
    }

    pub fn finalize(&mut self) {
        self.ended_at = current_iso8601();
    }
}

fn current_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("@{secs}")
}

/// Aggregate per-unit statuses into a single run status: all units failed →
/// `Failed`, any unit not `Success` → `Partial`, otherwise `Success`. An empty
/// report set is `Success`. Shared by the CLI (`kenn index`) and the workflow /
/// MCP `index_workspace` path so both classify a run identically.
#[must_use]
pub fn aggregate_status(reports: &[RunReport]) -> RunStatus {
    if reports.is_empty() {
        return RunStatus::Success;
    }
    if reports
        .iter()
        .all(|r| matches!(r.status, RunStatus::Failed))
    {
        return RunStatus::Failed;
    }
    if reports
        .iter()
        .any(|r| !matches!(r.status, RunStatus::Success))
    {
        return RunStatus::Partial;
    }
    RunStatus::Success
}

/// The out-of-root tripwire: a run that dropped ≥1 document outside the
/// workspace root (`out_of_root_seen`) and kept none (`files_seen`) — the
/// snapshot would be empty. Returns the dropped total when it fires, else
/// `None`. Deliberately `dropped > 0 && kept == 0`, so an honestly-empty run
/// (empty repo, all-excluded, or a producer that emitted nothing — all with
/// zero drops) is not caught, and a multi-producer run where one unit all-drops
/// but another yields in-root documents stays partial. Shared by `kenn index`
/// and the orchestrated `index_workspace` path so both treat an all-out-of-root
/// run identically.
#[must_use]
pub fn all_documents_outside_root(reports: &[RunReport]) -> Option<u64> {
    let dropped: u64 = reports.iter().map(|r| r.out_of_root_seen).sum();
    let kept: u64 = reports.iter().map(|r| r.files_seen).sum();
    (dropped > 0 && kept == 0).then_some(dropped)
}

/// Flatten the failed-project names each unit reported.
#[must_use]
pub fn collect_failed_projects(reports: &[RunReport]) -> Vec<String> {
    reports
        .iter()
        .flat_map(|r| r.failed_projects.iter().cloned())
        .collect()
}

/// Sum the attributions each unit dropped past its retention cap.
#[must_use]
pub fn collect_failed_overflow(reports: &[RunReport]) -> u64 {
    reports.iter().map(|r| r.failed_overflow).sum()
}

/// Flatten the warning diagnostics each unit reported.
#[must_use]
pub fn collect_warnings(reports: &[RunReport]) -> Vec<String> {
    reports
        .iter()
        .flat_map(|r| r.warnings.iter().cloned())
        .collect()
}

/// Sum the warning attributions dropped past per-unit retention caps.
#[must_use]
pub fn collect_warnings_overflow(reports: &[RunReport]) -> u64 {
    reports.iter().map(|r| r.warnings_overflow).sum()
}

/// Render a bounded attribution list for display, appending the `+N more`
/// overflow marker. The marker exists only in rendered output — counting
/// consumers use the list length plus the structured overflow.
#[must_use]
pub fn render_with_overflow(entries: &[String], overflow: u64) -> Vec<String> {
    let mut out = entries.to_vec();
    if overflow > 0 {
        out.push(format!("+{overflow} more"));
    }
    out
}

/// Render the run-local `overview.md` orientation snapshot from a snapshot's
/// primitive fields (no DB query). Shared so the CLI and workflow / MCP paths
/// emit an identical overview.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "one flat overview line per snapshot field; grouping them into a struct just to satisfy the lint adds indirection for no gain"
)]
pub fn render_overview_md(
    timestamp: &str,
    status: &str,
    documents: u64,
    symbols: u64,
    definitions: u64,
    edges: u64,
    source_root: &str,
    failed_projects: &[String],
    toolchains: &[ToolchainVersion],
) -> String {
    let failed = if failed_projects.is_empty() {
        "none".to_owned()
    } else {
        failed_projects.join(", ")
    };
    let provisioned = render_toolchains(toolchains);
    format!(
        "# Workspace overview\n\n\
         Generated by `kenn index` for the current snapshot — read this to orient. \
         If this file is absent, no snapshot is built yet: call `get_index_status`, \
         then `get_workspace_overview`. If `indexed_at` is far in the past, the index \
         may be stale — verify with `get_index_status`.\n\n\
         - indexed_at: {timestamp}\n\
         - status: {status}\n\
         - files: {documents}\n\
         - symbols: {symbols}\n\
         - definitions: {definitions}\n\
         - edges: {edges}\n\
         - source_root: {source_root}\n\
         - failed_projects: {failed}\n\
         - toolchains: {provisioned}\n",
    )
}

/// Render provisioned toolchains as `lang version, lang version`, or `none`.
/// Shared by `overview.md` and the CLI/`kenn status` run summary so the two
/// never drift.
#[must_use]
pub fn render_toolchains(toolchains: &[ToolchainVersion]) -> String {
    if toolchains.is_empty() {
        return "none".to_owned();
    }
    toolchains
        .iter()
        .map(|t| format!("{} {}", t.language, t.version))
        .collect::<Vec<_>>()
        .join(", ")
}
