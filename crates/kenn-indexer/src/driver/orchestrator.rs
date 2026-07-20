//! The cross-language `IndexerDriver` orchestrator plus the SCIP
//! output-path allocator its per-unit drivers share.

use std::path::PathBuf;

use crate::canonicalize::Workspace;
use crate::report::{RunReport, RunStatus};

use super::{DriverError, JsonlOutcome, ScipOutcome};

/// Allocate a deterministic SCIP output path — a derived intermediate,
/// resolved through the store layout (`Workspace::scip_path`). Reused
/// across runs (overwritten each run); the per-driver `slug` suffix
/// prevents collision between rust-analyzer and scip-typescript.
pub(crate) fn make_scip_output_path(
    workspace: &Workspace,
    slug: &str,
) -> Result<PathBuf, DriverError> {
    let derived_root = workspace.derived_root();
    std::fs::create_dir_all(derived_root).map_err(|e| {
        DriverError::Subprocess(format!(
            "create derived store dir {}: {e}",
            derived_root.display()
        ))
    })?;
    Ok(workspace.scip_path(slug))
}

/// Cross-language orchestrator. Holds two parallel containers because
/// SCIP and JSONL indexers have fundamentally different invocation
/// shapes (per-unit vs whole-workspace).
pub struct IndexerDriver {
    pub workspace: Workspace,
    pub scip_drivers: Vec<Box<dyn super::ScipDriver>>,
    pub jsonl_drivers: Vec<Box<dyn super::JsonlIndexer>>,
    /// Markdown corpus config when `[language.markdown] enabled = true`.
    /// `None` leaves markdown unindexed. Markdown is a sibling producer
    /// (design D1), not a SCIP/JSONL driver, so it has its own slot.
    pub markdown: Option<kenn_config::MarkdownConfig>,
    /// Stylesheet corpus config when `[language.css] enabled = true`. `None`
    /// leaves stylesheets unindexed. Like markdown, a sibling producer with
    /// its own slot.
    pub css: Option<kenn_config::CssConfig>,
    /// HTML corpus config when `[language.html] enabled = true`. `None` leaves
    /// HTML unindexed. A sibling producer whose connective edges resolve on the
    /// post-code/CSS barrier (design D4), so it shares css's slot shape.
    pub html: Option<kenn_config::HtmlConfig>,
    /// Text-fallback producer when `[language.text] enabled = true`. `None`
    /// leaves non-semantic text files unindexed. A barrier-free sibling
    /// producer (no link graph), carrying the claimed-extension skip set.
    pub text: Option<crate::text::TextCorpus>,
    /// Docker cache volumes any `runtime = "docker"` language uses, for the
    /// preflight to create + label + chown before the first `--user` container.
    /// Empty unless a language opts into the docker runtime.
    pub docker_cache_volumes: Vec<crate::docker::CacheVolume>,
    /// `(image, version)` for a Swift toolchain to place in the cache during
    /// preflight. Swift is the ONLY language provisioned host-side: swift.org
    /// publishes no verifiable download, so its toolchain is copied out of the
    /// official image — and only the host can call docker.
    pub swift_toolchain: Option<(String, String)>,
}

impl IndexerDriver {
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            scip_drivers: Vec::new(),
            jsonl_drivers: Vec::new(),
            markdown: None,
            css: None,
            html: None,
            text: None,
            docker_cache_volumes: Vec::new(),
            swift_toolchain: None,
        }
    }

    /// Enable markdown corpus indexing with `config`.
    #[must_use]
    pub fn with_markdown(mut self, config: kenn_config::MarkdownConfig) -> Self {
        self.markdown = Some(config);
        self
    }

    /// Enable stylesheet (css/sass) corpus indexing with `config`.
    #[must_use]
    pub fn with_css(mut self, config: kenn_config::CssConfig) -> Self {
        self.css = Some(config);
        self
    }

    /// Enable HTML corpus indexing with `config`.
    #[must_use]
    pub fn with_html(mut self, config: kenn_config::HtmlConfig) -> Self {
        self.html = Some(config);
        self
    }

    /// Enable the text-fallback producer with `config`. `claimed_exts` are the
    /// extensions enabled producers own, so the fallback skips files another
    /// indexer already handles (no double-indexing).
    #[must_use]
    pub fn with_text(
        mut self,
        config: kenn_config::TextConfig,
        claimed_exts: std::collections::BTreeSet<String>,
    ) -> Self {
        self.text = Some(crate::text::TextCorpus {
            config,
            claimed_exts,
        });
        self
    }

    #[must_use]
    pub fn with_scip_driver<D: super::ScipDriver + 'static>(mut self, driver: D) -> Self {
        self.scip_drivers.push(Box::new(driver));
        self
    }

    #[must_use]
    pub fn with_jsonl_driver<I: super::JsonlIndexer + 'static>(mut self, indexer: I) -> Self {
        self.jsonl_drivers.push(Box::new(indexer));
        self
    }

    /// Run every registered driver/indexer and collect their reports.
    /// Note: `JsonlOutcome::Jsonl` outcomes carry a running subprocess
    /// the pipeline must consume — this function drops them and only
    /// returns the report. Callers wanting streaming ingestion should
    /// use `run_pipeline` instead.
    #[must_use]
    pub fn run_all(&self) -> Vec<RunReport> {
        let mut reports = Vec::new();
        for driver in &self.scip_drivers {
            let units = match driver.discover_units(&self.workspace) {
                Ok(u) => u,
                Err(e) => {
                    let mut report = RunReport::started(driver.language_id(), "?", "<discover>");
                    report.status = RunStatus::Failed;
                    report.failed_projects.push(format!("discover: {e}"));
                    report.finalize();
                    reports.push(report);
                    continue;
                }
            };
            for unit in units {
                match driver.run_unit(&unit, &self.workspace) {
                    Ok(ScipOutcome::Scip { report, .. } | ScipOutcome::Unavailable { report }) => {
                        reports.push(report);
                    }
                    Err(e) => {
                        let mut report =
                            RunReport::started(driver.language_id(), "?", &unit.identifier);
                        report.status = RunStatus::Failed;
                        report.failed_projects.push(format!("{e}"));
                        report.finalize();
                        reports.push(report);
                    }
                }
            }
        }
        for indexer in &self.jsonl_drivers {
            match indexer.run(&self.workspace) {
                Ok(JsonlOutcome::Jsonl { report, .. } | JsonlOutcome::Unavailable { report }) => {
                    reports.push(report);
                }
                Err(e) => {
                    let mut report = RunReport::started(
                        indexer.language_id(),
                        "?",
                        &format!(
                            "{}@{}",
                            indexer.language_id(),
                            self.workspace.root().display()
                        ),
                    );
                    report.status = RunStatus::Failed;
                    report.failed_projects.push(format!("{e}"));
                    report.finalize();
                    reports.push(report);
                }
            }
        }
        reports
    }
}
