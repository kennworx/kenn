//! Per-unit ingest: one OS thread per language driver. Each owns its
//! `IdRegistry` partition and `BatchSink`, parses its SCIP/JSONL stream,
//! and appends records directly to the store — plus the
//! kenn-dotnet crash-retry and subprocess-status plumbing.

use std::path::{Path, PathBuf};
use std::process::Child;

use kenn_model::{DefRecord, Language};

use crate::canonicalize::Workspace;
use crate::driver::{JsonlIndexer, JsonlOutcome, ScipDriver, ScipOutcome};
use crate::parse::{parse_scip_stream_with_metadata, ParseError};
use crate::report::{RunReport, RunStatus};
use crate::sink::BatchSink;
use crate::transform::{transform_document, IdRegistry, TransformError};
use crate::transform_jsonl::flush_registry_stubs;

use super::{IngestUnit, PipelineError, ProgressEvent};

/// Count definitions carrying an enclosing-item body extent
/// (`body_end_line >= 1`) — the rust-analyzer-capability signal (see
/// [`UnitCounts::def_bodies`]).
fn count_body_extents(defs: &[DefRecord]) -> u64 {
    defs.iter().filter(|d| d.body_end_line >= 1).count() as u64
}

#[derive(Debug, Default)]
pub(crate) struct UnitCounts {
    pub(crate) files: u64,
    pub(crate) symbols: u64,
    pub(crate) defs: u64,
    /// Definitions that carried an enclosing-item body extent. Drives the
    /// rust-analyzer-too-old warning: a Rust unit with `defs > 0` but
    /// `def_bodies == 0` means the resolved rust-analyzer emits no
    /// `enclosing_range` (a pre-Dec-2025 build).
    pub(crate) def_bodies: u64,
    pub(crate) edges: u64,
    /// `ErrorFrame{severity: "error"}` frames observed on a JSONL stream.
    /// Always 0 on the SCIP path. Excludes a run from the crash-retry
    /// signature: an attributed deterministic failure is not the
    /// `BuildHost` startup race. Warnings deliberately don't count —
    /// sidecars emit them during enumeration, before the crash window.
    pub(crate) frame_errors: u64,
}

/// Ingest every unit of one SCIP driver. Runs on its own OS thread, owns
/// its own `IdRegistry` (its language partition) and its own
/// [`BatchSink`], and appends records directly to the store.
/// Returns the per-unit reports and the count of registry stubs flushed.
pub(crate) fn ingest_scip_driver<F>(
    driver: &dyn ScipDriver,
    workspace: &Workspace,
    mut sink: BatchSink,
    progress: &F,
) -> (Vec<RunReport>, u64)
where
    F: Fn(ProgressEvent) + Sync,
{
    let mut reports = Vec::new();
    let units = match driver.discover_units(workspace) {
        Ok(u) => u,
        Err(e) => {
            let mut report = RunReport::started(driver.language_id(), "?", "<discover>");
            report.status = RunStatus::Failed;
            report.failed_projects.push(format!("discover: {e}"));
            report.finalize();
            return (vec![report], 0);
        }
    };
    let Some(language) = Language::from_db_name(driver.language_id()) else {
        for unit in units {
            match driver.run_unit(&unit, workspace) {
                Ok(ScipOutcome::Scip { report, .. } | ScipOutcome::Unavailable { report }) => {
                    reports.push(report);
                }
                Err(e) => {
                    reports.push(failed_unit_report(
                        driver.language_id(),
                        &unit.identifier,
                        &e,
                    ));
                }
            }
        }
        return (reports, 0);
    };

    let mut registry = IdRegistry::new(language);
    for unit in units {
        let outcome = match driver.run_unit(&unit, workspace) {
            Ok(o) => o,
            Err(e) => {
                reports.push(failed_unit_report(
                    driver.language_id(),
                    &unit.identifier,
                    &e,
                ));
                continue;
            }
        };
        match outcome {
            ScipOutcome::Scip { path, mut report } => {
                if matches!(report.status, RunStatus::Failed) {
                    reports.push(report);
                    continue;
                }
                let counts =
                    ingest_scip_into_sink(&path, workspace, &mut registry, &mut sink, &mut report);
                if let Ok(c) = counts.as_ref() {
                    progress(ProgressEvent::UnitIngested {
                        language,
                        unit: IngestUnit::Scip(unit.clone()),
                        files: c.files,
                        symbols: c.symbols,
                        edges: c.edges,
                    });
                }
                finalize_unit(counts, &mut report, &mut reports);
            }
            ScipOutcome::Unavailable { report } => reports.push(report),
        }
    }
    let stubs = flush_registry_stubs(&mut registry, &mut sink).unwrap_or(0);
    if let Err(e) = sink.finish() {
        reports.push(failed_unit_report(driver.language_id(), "<finalize>", &e));
    }
    (reports, stubs)
}

/// Ingest one JSONL driver's whole-workspace run on its own OS thread.
pub(crate) fn ingest_jsonl_driver<F>(
    driver: &dyn JsonlIndexer,
    workspace: &Workspace,
    sink: BatchSink,
    progress: &F,
) -> (Vec<RunReport>, u64)
where
    F: Fn(ProgressEvent) + Sync,
{
    let outcome = match driver.run(workspace) {
        Ok(o) => o,
        Err(e) => {
            let identifier = format!("{}@{}", driver.language_id(), workspace.root().display());
            return (
                vec![failed_unit_report(driver.language_id(), &identifier, &e)],
                0,
            );
        }
    };
    match outcome {
        JsonlOutcome::Jsonl {
            child,
            stream_path,
            stderr,
            report,
        } => ingest_jsonl_unit(
            driver,
            workspace,
            sink,
            progress,
            child,
            stream_path,
            stderr,
            report,
        ),
        JsonlOutcome::Unavailable { report } => (vec![report], 0),
    }
}

/// Run a JSONL stream through to the sink. Split out of
/// `ingest_jsonl_driver` so the dispatcher stays a thin shape-match
/// (its cyclomatic complexity stays low). Error short-circuits land
/// here together so the read path is one linear column.
#[expect(
    clippy::too_many_arguments,
    reason = "all eight inputs already exist at the caller; bundling them into a struct just shifts the noise"
)]
fn ingest_jsonl_unit<F>(
    driver: &dyn JsonlIndexer,
    workspace: &Workspace,
    mut sink: BatchSink,
    progress: &F,
    child: Child,
    stream_path: PathBuf,
    stderr: Option<crate::driver::StderrCapture>,
    mut report: RunReport,
) -> (Vec<RunReport>, u64)
where
    F: Fn(ProgressEvent) + Sync,
{
    if matches!(report.status, RunStatus::Failed) {
        drop(std::fs::remove_file(&stream_path));
        return (vec![report], 0);
    }
    let Some(language) = Language::from_db_name(driver.language_id()) else {
        drop(std::fs::remove_file(&stream_path));
        return (vec![report], 0);
    };
    let mut registry = IdRegistry::new(language);
    let mut reports = Vec::new();
    let counts = run_jsonl_with_retry(
        driver,
        workspace,
        child,
        stream_path,
        stderr,
        &mut registry,
        &mut sink,
        &mut report,
    );
    if let Ok(c) = counts.as_ref() {
        progress(ProgressEvent::UnitIngested {
            language,
            unit: IngestUnit::JsonlWorkspace,
            files: c.files,
            symbols: c.symbols,
            edges: c.edges,
        });
    }
    finalize_unit(counts, &mut report, &mut reports);
    let stubs = flush_registry_stubs(&mut registry, &mut sink).unwrap_or(0);
    if let Err(e) = sink.finish() {
        reports.push(failed_unit_report(driver.language_id(), "<finalize>", &e));
    }
    (reports, stubs)
}

fn failed_unit_report(lang: &str, identifier: &str, err: &dyn std::fmt::Display) -> RunReport {
    let mut report = RunReport::started(lang, "?", identifier);
    report.status = RunStatus::Failed;
    report.failed_projects.push(format!("{err}"));
    report.finalize();
    report
}

/// Fold a SCIP `Metadata` frame into the unit's mutable state: the indexer's
/// self-reported version, and the project root the documents' relative paths
/// resolve against. Both are optional in the wire format.
fn absorb_scip_metadata(
    meta: scip::types::Metadata,
    project_root_uri: &std::cell::RefCell<String>,
    observed_version: &mut Option<String>,
) {
    if let Some(tool) = meta.tool_info.as_ref() {
        if !tool.version.is_empty() {
            *observed_version = Some(tool.version.clone());
        }
    }
    if !meta.project_root.is_empty() {
        *project_root_uri.borrow_mut() = meta.project_root;
    }
}

fn ingest_scip_into_sink(
    scip_path: &Path,
    workspace: &Workspace,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
    report: &mut RunReport,
) -> Result<UnitCounts, PipelineError> {
    use crate::edge::{derive_edges_for_document, is_definition};
    use crate::enclosing::BareLastPrecedingDef;
    use crate::transform::language_from_scip;
    use kenn_model::EdgeRecord;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    // Pass 1 — build a workspace-wide def-count map for this scip file.
    let mut def_counts: HashMap<String, usize> = HashMap::new();
    {
        let file = std::fs::File::open(scip_path)?;
        let mut reader = std::io::BufReader::new(file);
        crate::parse::parse_scip_stream(&mut reader, |doc| {
            for occ in &doc.occurrences {
                if is_definition(occ) {
                    *def_counts.entry(occ.symbol.clone()).or_insert(0) += 1;
                }
            }
            Ok(())
        })?;
    }

    // Pass 2 — emit records and derive per-document occurrence edges.
    let file = std::fs::File::open(scip_path)?;
    let mut reader = std::io::BufReader::new(file);
    let project_root_uri = RefCell::new(format!("file://{}", workspace.root().display()));
    let counts = RefCell::new(UnitCounts::default());
    // Out-of-root drops + first sample (SCIP-only — JSONL emits relative paths).
    let out_of_root = RefCell::new(OutOfRootTally::default());
    let mut observed_version: Option<String> = None;

    let parsed = parse_scip_stream_with_metadata(
        &mut reader,
        |meta| {
            absorb_scip_metadata(meta, &project_root_uri, &mut observed_version);
            Ok(())
        },
        |doc| {
            let pru = project_root_uri.borrow();
            let mut transformed = match transform_document(&doc, workspace, &pru, registry) {
                Ok(t) => t,
                Err(e) => match classify_transform_error(e)? {
                    DroppedDoc::Silent => return Ok(()),
                    DroppedDoc::OutOfRoot { path, root } => {
                        out_of_root.borrow_mut().note(path, root);
                        return Ok(());
                    }
                },
            };
            drop(pru);
            let language = language_from_scip(&doc.language).or_else(|| {
                doc.language
                    .is_empty()
                    .then(|| crate::transform::language_from_path(&doc.relative_path))
                    .flatten()
            });
            if let Some(lang) = language {
                if !matches!(lang, kenn_model::Language::Csharp) {
                    let mut occurrence_edges: HashSet<EdgeRecord> =
                        transformed.edges.iter().cloned().collect();
                    let mut enclosing = BareLastPrecedingDef;
                    let workspace_path = transformed.file.as_ref().map_or("", |f| f.path.as_str());
                    derive_edges_for_document(
                        &doc,
                        workspace_path,
                        &mut enclosing,
                        registry,
                        lang,
                        &|sym: &str| def_counts.get(sym).copied().unwrap_or(0),
                        &mut occurrence_edges,
                    );
                    transformed.edges = occurrence_edges.into_iter().collect();
                }
            }
            let mut c = counts.borrow_mut();
            if transformed.file.is_some() {
                c.files += 1;
            }
            c.symbols += transformed.symbols.len() as u64;
            c.defs += transformed.defs.len() as u64;
            c.def_bodies += count_body_extents(&transformed.defs);
            c.edges += transformed.edges.len() as u64;
            drop(c);
            for d in transformed.file_docs {
                sink.push_file_docs(d)
                    .map_err(|e| ParseError::Io(std::io::Error::other(format!("sink: {e}"))))?;
            }
            sink.push_document_records(
                transformed.file,
                transformed.symbols,
                transformed.docs,
                transformed.defs,
                transformed.edges,
            )
            .map_err(|e| ParseError::Io(std::io::Error::other(format!("sink: {e}"))))?;
            Ok(())
        },
    );

    // Applied before `?`: the metadata frame precedes every document, so a
    // stream that fails midway has already told us who produced it. The failing
    // unit's report is the one a human reads, and `"?"` there is a dead end.
    apply_observed_version(observed_version, report);
    record_out_of_root(out_of_root.into_inner(), report);
    parsed?;
    Ok(counts.into_inner())
}

/// A document `transform_document` could not place. `Silent` drops are
/// intentional (unclassifiable language, or an excluded path); `OutOfRoot` is a
/// `project_root`/workspace-root mismatch worth counting and reporting.
enum DroppedDoc {
    Silent,
    OutOfRoot { path: String, root: String },
}

/// Classify a `transform_document` error into an intentional drop, an
/// out-of-root drop, or a hard error that aborts the SCIP stream.
fn classify_transform_error(err: TransformError) -> Result<DroppedDoc, ParseError> {
    use crate::canonicalize::CanonicalizeError;
    match err {
        TransformError::UnknownLanguage(_)
        | TransformError::Canonicalize(CanonicalizeError::Excluded(_)) => Ok(DroppedDoc::Silent),
        TransformError::Canonicalize(CanonicalizeError::OutsideRoot { path, root }) => {
            Ok(DroppedDoc::OutOfRoot { path, root })
        }
        e => Err(ParseError::Io(std::io::Error::other(format!(
            "transform: {e}"
        )))),
    }
}

/// Running tally of one SCIP unit's out-of-root drops: the count and the first
/// mismatch (path vs root) for the report.
#[derive(Default)]
struct OutOfRootTally {
    count: u64,
    sample: Option<(String, String)>,
}

impl OutOfRootTally {
    fn note(&mut self, path: String, root: String) {
        self.count += 1;
        self.sample.get_or_insert((path, root));
    }
}

/// Fold out-of-root drops into the unit's report: the count, one warning naming
/// the first mismatch, and a status degrade to `Partial` (the snapshot is now
/// missing those documents). The run-level "every document dropped ⇒ fail"
/// decision is the caller's — a unit that also produced in-root documents is a
/// partial, not a failure.
fn record_out_of_root(tally: OutOfRootTally, report: &mut RunReport) {
    let OutOfRootTally { count, sample } = tally;
    if count == 0 {
        return;
    }
    report.out_of_root_seen = count;
    if let Some((path, root)) = sample {
        report.warnings.push(format!(
            "{count} document(s) fell outside the workspace root — e.g. `{path}` \
             is not under `{root}`"
        ));
    }
    if !matches!(report.status, RunStatus::Failed) {
        report.status = RunStatus::Partial;
    }
}

/// Replace the driver's `"?"` placeholder with the version the producer
/// reported, if it reported one. A silent producer leaves `"?"` alone: unknown
/// reads better than an empty string.
fn apply_observed_version(observed: Option<String>, report: &mut RunReport) {
    if let Some(v) = observed {
        report.indexer_version = v;
    }
}

/// Roslyn 4.7's `MSBuildWorkspace` `BuildHost` has a flaky
/// `AccessViolationException` race on macOS/Linux: `kenn-dotnet` exits 134
/// (SIGABRT) with zero JSONL frames. We retry up to `MAX_JSONL_RETRIES`
/// times on this exact signature; everything else is reported as-is.
const MAX_JSONL_RETRIES: u32 = 3;

#[expect(
    clippy::too_many_arguments,
    reason = "retry wraps a many-input ingest call; splitting just to placate the lint hurts readability"
)]
pub(crate) fn run_jsonl_with_retry(
    indexer: &dyn JsonlIndexer,
    workspace: &Workspace,
    initial_child: Child,
    initial_stream_path: PathBuf,
    initial_stderr: Option<crate::driver::StderrCapture>,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
    report: &mut RunReport,
) -> Result<UnitCounts, PipelineError> {
    let mut child = initial_child;
    let mut stream_path = initial_stream_path;
    let mut stderr = initial_stderr;
    for attempt in 0..=MAX_JSONL_RETRIES {
        let mut attempt_report = report.clone();
        let counts = ingest_jsonl_subprocess(
            child,
            &stream_path,
            stderr,
            workspace,
            registry,
            sink,
            &mut attempt_report,
        )?;
        cleanup_jsonl_stream(&stream_path);
        if !is_jsonl_crash_with_no_data(&attempt_report, &counts) || attempt == MAX_JSONL_RETRIES {
            *report = attempt_report;
            return Ok(counts);
        }
        if let Some((c, p, e)) = attempt_jsonl_respawn(indexer, workspace) {
            child = c;
            stream_path = p;
            stderr = e;
        } else {
            *report = attempt_report;
            return Ok(counts);
        }
    }
    #[expect(
        clippy::unreachable,
        reason = "loop is bounded by 0..=MAX and the final iteration unconditionally returns"
    )]
    {
        unreachable!("retry loop exits via return")
    }
}

/// Drop the JSONL stream file unless `KENN_KEEP_JSONL` asks to keep it
/// (debug aid for the consumer/producer handoff).
fn cleanup_jsonl_stream(stream_path: &Path) {
    if std::env::var_os("KENN_KEEP_JSONL").is_none() {
        drop(std::fs::remove_file(stream_path));
    }
}

/// Detect the SIGABRT-with-no-frames signature that the Roslyn 4.7
/// `BuildHost` race produces. Anything else is reported as-is. A stream
/// that carried `severity: "error"` frames is excluded — a deterministic
/// failure was already attributed. Warning frames do NOT exclude: the
/// sidecars emit warnings during enumeration, before the crash-prone
/// project build even starts, so a warning proves nothing about the race.
pub(crate) fn is_jsonl_crash_with_no_data(attempt_report: &RunReport, counts: &UnitCounts) -> bool {
    matches!(attempt_report.status, RunStatus::Partial)
        && counts.files == 0
        && counts.symbols == 0
        && counts.edges == 0
        && counts.frame_errors == 0
}

/// Try to respawn the producer for one more retry. `None` when the
/// indexer reports `Unavailable` or errors — caller stops the loop.
fn attempt_jsonl_respawn(
    indexer: &dyn JsonlIndexer,
    workspace: &Workspace,
) -> Option<(Child, PathBuf, Option<crate::driver::StderrCapture>)> {
    match indexer.run(workspace) {
        Ok(JsonlOutcome::Jsonl {
            child,
            stream_path,
            stderr,
            ..
        }) => Some((child, stream_path, stderr)),
        _ => None,
    }
}

fn ingest_jsonl_subprocess(
    mut child: Child,
    stream_path: &Path,
    stderr: Option<crate::driver::StderrCapture>,
    workspace: &Workspace,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
    report: &mut RunReport,
) -> Result<UnitCounts, PipelineError> {
    let (stats, _end_frame_seen) = crate::transform_jsonl::ingest_jsonl_from_growing_file(
        stream_path,
        &mut child,
        workspace,
        registry,
        sink,
    )?;
    // The driver seeds `indexer_version` with `"?"`; the `meta` frame has since
    // told us who the producer actually is.
    if let Some(v) = &stats.tool_version {
        report.indexer_version.clone_from(v);
    }
    record_frame_diagnostics(&stats, report);
    let stderr_tail = drain_stderr_capture(stderr);
    record_jsonl_exit_status(child.wait(), &stderr_tail, report);
    Ok(UnitCounts {
        files: stats.files,
        symbols: stats.symbols,
        defs: stats.defs,
        // JSONL producers (C#/TS) emit body extents unconditionally, not
        // gated on a tool version, so the rust-analyzer-capability warning
        // never consults this for them — left 0 rather than threaded through
        // `IngestStats`.
        def_bodies: 0,
        edges: stats.edges,
        frame_errors: stats.failed_errors,
    })
}

/// Surface the stream's diagnostics in the unit's report. Error-severity
/// attributions extend `failed_projects` (overflow past the cap recorded
/// as structured `failed_overflow`, rendered `+N more` only at display
/// time) and degrade the status to `Partial` unless it is already
/// `Failed` (jsonl-indexer-driver spec). Warning-severity attributions
/// extend `warnings` the same way, status-neutral — they carry producer
/// degradation notices (e.g. stale index-store units) that `kenn status`
/// must show.
pub(crate) fn record_frame_diagnostics(
    stats: &crate::transform_jsonl::JsonlIngestStats,
    report: &mut RunReport,
) {
    if stats.warning_total > 0 {
        report.warnings.extend(stats.warned.iter().cloned());
        report.warnings_overflow += stats
            .warning_total
            .saturating_sub(stats.warned.len() as u64);
    }
    if stats.failed_errors == 0 {
        return;
    }
    report.failed_projects.extend(stats.failed.iter().cloned());
    report.failed_overflow += stats
        .failed_errors
        .saturating_sub(stats.failed.len() as u64);
    if !matches!(report.status, RunStatus::Failed) {
        report.status = RunStatus::Partial;
    }
}

/// Drain the producer's stderr capture: return the tail (≤8 KiB) for
/// the error message, then join the reader thread.
fn drain_stderr_capture(stderr: Option<crate::driver::StderrCapture>) -> String {
    let Some(c) = stderr else {
        return String::new();
    };
    let tail = c.tail(8 * 1024);
    drop(c.handle.join());
    tail
}

/// Map the subprocess exit status onto `report.status` + `failed_projects`.
/// Non-zero exit or a `wait()` error degrades the report to Partial. The
/// failure message names the report's producer (`indexer_name`), which is
/// stable across runner-form command configs (`["dotnet", "kenn-dotnet.dll"]`).
pub(crate) fn record_jsonl_exit_status(
    exit: std::io::Result<std::process::ExitStatus>,
    stderr_tail: &str,
    report: &mut RunReport,
) {
    match exit {
        Ok(s) if !s.success() => {
            report.status = RunStatus::Partial;
            let code = s
                .code()
                .map_or_else(|| format!("signal/{s:?}"), |c| format!("exit {c}"));
            let mut msg = format!("{} {code}", report.indexer_name);
            if !stderr_tail.is_empty() {
                // Lead with the extracted cause, then keep the tail.
                //
                // This path used to append the raw 8KB tail alone, so the one
                // actionable sentence sat buried in build noise — fine for a
                // human scrolling, useless to an agent reading the failure over
                // a tool call. `error_reason` is the same extraction the SCIP
                // drivers already use, and it prefers the first `error` line
                // over the last, because the last is usually a backtrace frame.
                //
                // The tail is RETAINED rather than replaced: the two readers
                // want different things, and dropping it would trade one
                // information loss for another.
                let reason = crate::driver::error_reason(stderr_tail);
                if !reason.is_empty() {
                    msg.push_str(": ");
                    msg.push_str(reason);
                }
                msg.push_str("\nstderr tail:\n");
                msg.push_str(stderr_tail.trim_end());
            }
            report.failed_projects.push(msg);
        }
        Err(e) => {
            report.status = RunStatus::Partial;
            let msg = format!("{} wait failed: {e}", report.indexer_name);
            report.failed_projects.push(msg);
        }
        _ => {}
    }
}

/// Fold one unit's ingest result into its `RunReport`.
fn finalize_unit(
    counts: Result<UnitCounts, PipelineError>,
    report: &mut RunReport,
    reports: &mut Vec<RunReport>,
) {
    match counts {
        Ok(c) => {
            report.files_seen = c.files;
            report.symbols_seen = c.symbols;
            report.defs_seen = c.defs;
            report.def_bodies_seen = c.def_bodies;
            report.edges_seen = c.edges;
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(format!("ingest: {e}"));
        }
    }
    report.finalize();
    reports.push(report.clone());
}
