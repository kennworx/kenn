//! Public pipeline API: the progress/event/error types, the
//! `run_pipeline` / `run_pipeline_with_progress` orchestrators, and the
//! phase-1 preflight.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use kenn_model::{AggregateEdgeRecord, AggregateNodeRecord, Language};
use kenn_store::api::DbError;
use kenn_store::DbWriter;

use crate::driver::IndexerDriver;
use crate::parse::ParseError;
use crate::report::{RunReport, RunStatus};
use crate::sink::BatchSink;
use crate::transform::TransformError;
use crate::transform_jsonl::JsonlTransformError;

use super::{ingest_jsonl_driver, ingest_scip_driver};

/// The version reported by producers compiled into this crate — markdown, CSS,
/// HTML, text. Subprocess producers report their own, so a run report that
/// mixed a real version with a bare `"0"` read as though the sidecar had one.
const IN_PROCESS_PRODUCER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type PostAggregateHook = Box<
    dyn FnOnce(
            Vec<AggregateNodeRecord>,
            Vec<AggregateEdgeRecord>,
            DbWriter,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send>>
        + Send,
>;

/// No-op [`PostAggregateHook`] — for callers with no post-aggregate work.
#[must_use]
pub fn no_op_hook() -> PostAggregateHook {
    Box::new(|_nodes, _edges, _writer| Box::pin(async { Ok(()) }))
}

/// What kind of indexing unit produced a `ProgressEvent::UnitIngested`.
#[derive(Debug, Clone)]
pub enum IngestUnit {
    /// SCIP per-unit invocation.
    Scip(crate::driver::Unit),
    /// JSONL whole-workspace invocation.
    JsonlWorkspace,
}

/// Coarse-grained pipeline progress events surfaced to a caller-supplied
/// callback. The callback may be invoked from ingester threads, so it
/// must be `Sync`.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Pipeline started.
    Started,
    /// One language ingester's unit finished ingesting.
    UnitIngested {
        language: Language,
        unit: IngestUnit,
        files: u64,
        symbols: u64,
        edges: u64,
    },
    /// Cross-ingester stub flush total (post-ingest).
    StubsFlushed { count: u64 },
    /// Aggregation pass computed and persisted the `aggregate_*` tables.
    AggregateComputed {
        nodes: u64,
        edges: u64,
        elapsed_ms: u128,
    },
    /// Finalize finished — the snapshot is ready to publish.
    EndRunComplete { elapsed_ms: u128 },
    /// Pipeline terminated successfully.
    Completed { total_ms: u128 },
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] ParseError),
    #[error("transform: {0}")]
    Transform(#[from] TransformError),
    #[error("jsonl: {0}")]
    Jsonl(#[from] JsonlTransformError),
    #[error("sink: {0}")]
    Sink(#[from] DbError),
    #[error("preflight: {0}")]
    MissingCli(String),
    #[error("a pipeline worker thread panicked")]
    WorkerPanicked,
}

/// Run every language driver registered on `runner` through the 4-phase
/// orchestrator, writing into `writer`, and return the per-unit reports
/// plus the finalized writer.
///
/// # Errors
/// Fails in the prepare phase when a required ingester CLI is missing,
/// and on any backend write / finalize error.
pub fn run_pipeline(
    runner: &IndexerDriver,
    writer: DbWriter,
    batch_size: usize,
) -> Result<(Vec<RunReport>, DbWriter), PipelineError> {
    run_pipeline_with_progress(runner, writer, batch_size, |_| {}, no_op_hook(), None)
}

/// Like [`run_pipeline`] but with a caller-supplied progress callback and
/// a post-aggregate hook.
///
/// # Errors
/// See [`run_pipeline`].
#[expect(
    clippy::too_many_lines,
    reason = "the 4-phase orchestrator reads as one straight-line sequence (prepare → ingest → md-barrier → aggregate/finalize); splitting it would scatter the shared writer/handle/scope state"
)]
pub fn run_pipeline_with_progress<F>(
    runner: &IndexerDriver,
    writer: DbWriter,
    batch_size: usize,
    progress: F,
    post_aggregate_hook: PostAggregateHook,
    atlas: Option<crate::atlas::producer::AtlasContext>,
) -> Result<(Vec<RunReport>, DbWriter), PipelineError>
where
    F: Fn(ProgressEvent) + Sync,
{
    let t0 = std::time::Instant::now();
    let bench = *crate::BENCH_ENABLED;

    // ── Phase 1: prepare ────────────────────────────────────────────
    preflight(runner)?;
    let layout = crate::package_layout::PackageLayout::discover(
        runner.workspace.root(),
        runner.workspace.excluded_dirs(),
    );
    progress(ProgressEvent::Started);

    // The Lance store is async; this pipeline runs inside the caller's
    // `spawn_blocking`, so it owns a private runtime. The ingester OS
    // threads carry no runtime context, so `Handle::block_on` on them is
    // safe; the aggregate / finalize step runs on one more such thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| PipelineError::Sink(DbError::Backend(format!("build runtime: {e}"))))?;
    let handle = runtime.handle().clone();

    // ── Phase 2: ingest ─────────────────────────────────────────────
    let workspace = &runner.workspace;
    let progress_ref = &progress;
    let has_code = !runner.scip_drivers.is_empty() || !runner.jsonl_drivers.is_empty();
    let (mut reports, stubs, md_pending, css_pending, html_pending) =
        std::thread::scope(|scope| {
            let mut joins = Vec::new();
            for driver in &runner.scip_drivers {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                joins.push(scope.spawn(move || {
                    ingest_scip_driver(driver.as_ref(), workspace, sink, progress_ref)
                }));
            }
            for driver in &runner.jsonl_drivers {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                joins.push(scope.spawn(move || {
                    ingest_jsonl_driver(driver.as_ref(), workspace, sink, progress_ref)
                }));
            }
            // Markdown phase 1 (design D1/6.1) — md↔md nodes + links — runs as a
            // sibling unit. Its md→code resolution waits for the code join barrier
            // below (design D4/6.2); the deferred links ride out in `MarkdownPending`.
            let md_join = runner.markdown.as_ref().map(|md_cfg| {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                let root = workspace.root();
                scope.spawn(move || markdown_phase1_unit(md_cfg, root, sink))
            });
            // Stylesheet producer (css/sass) — another sibling unit. Its
            // class-usage mining waits for the post-code barrier (the deferred
            // usage files ride out in `CssPending`).
            let css_join = runner.css.as_ref().map(|css_cfg| {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                let root = workspace.root();
                scope.spawn(move || css_phase1_unit(css_cfg, root, sink))
            });
            // HTML producer — another sibling unit. Emits the document/id/inline
            // nodes; its links/imports/correspondence/class-usage resolve on the
            // post-code/CSS barrier (the deferred state rides out in `HtmlPending`).
            let html_join = runner.html.as_ref().filter(|c| c.enabled).map(|html_cfg| {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                let root = workspace.root();
                scope.spawn(move || html_phase1_unit(html_cfg, root, sink))
            });
            // SQL producer — a barrier-free sibling unit: it resolves table
            // identity inside its own pass over the `.sql` files it walks, so
            // there is no pending state for the post-code barrier.
            let sql_join = runner.sql.as_ref().map(|sql_cfg| {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                let root = workspace.root();
                scope.spawn(move || sql_unit(sql_cfg, root, sink))
            });
            // XML producer — another barrier-free sibling unit.
            let xml_join = runner.xml.as_ref().map(|xml_cfg| {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                let root = workspace.root();
                scope.spawn(move || xml_unit(xml_cfg, root, sink))
            });
            // Text-fallback producer — a barrier-free sibling unit (no link
            // graph, no post-code phase); its records are complete on join.
            let text_join = runner.text.as_ref().map(|corpus| {
                let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
                let root = workspace.root();
                scope.spawn(move || text_unit(corpus, root, sink))
            });
            let mut reports = Vec::new();
            let mut stubs = 0u64;
            for h in joins {
                // A clean ingester join is the clean-stream signal (design
                // D9) — a panic is a truncated run.
                match h.join() {
                    Ok((r, s)) => {
                        reports.extend(r);
                        stubs += s;
                    }
                    Err(_) => reports.push(panicked_report()),
                }
            }
            let md_pending = md_join.and_then(|h| {
                if let Ok((r, pending)) = h.join() {
                    reports.extend(r);
                    pending
                } else {
                    reports.push(panicked_report());
                    None
                }
            });
            let css_pending = css_join.and_then(|h| {
                if let Ok((r, pending)) = h.join() {
                    reports.extend(r);
                    pending
                } else {
                    reports.push(panicked_report());
                    None
                }
            });
            let html_pending = html_join.and_then(|h| {
                if let Ok((r, pending)) = h.join() {
                    reports.extend(r);
                    pending
                } else {
                    reports.push(panicked_report());
                    None
                }
            });
            // Neither SQL nor text has a post-code phase: join both here.
            if let Some(h) = sql_join {
                match h.join() {
                    Ok(r) => reports.push(r),
                    Err(_) => reports.push(panicked_report()),
                }
            }
            if let Some(h) = xml_join {
                match h.join() {
                    Ok(r) => reports.push(r),
                    Err(_) => reports.push(panicked_report()),
                }
            }
            if let Some(h) = text_join {
                match h.join() {
                    Ok(r) => reports.extend(r),
                    Err(_) => reports.push(panicked_report()),
                }
            }
            (reports, stubs, md_pending, css_pending, html_pending)
        });
    progress(ProgressEvent::StubsFlushed { count: stubs });

    // ── Phase 2b: md→code post-code barrier (design D4/D6) ───────────
    // All code units have joined, so their symbols/files now exist in the
    // building store. Resolve the deferred in-repo markdown links against it
    // (a code-less run dangles them) before aggregate/finalize.
    if let Some(pending) = md_pending {
        let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
        reports.push(resolve_markdown_code_unit(
            pending,
            &writer,
            &handle,
            has_code,
            workspace.root(),
            sink,
        ));
    }

    // ── Phase 2c: css class-usage post-code barrier ──────────────────
    // Class-usage edges attach to code symbols, which now exist; resolve the
    // deferred usage-source files against the building store.
    if let Some(pending) = css_pending {
        let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
        reports.push(resolve_css_usage_unit(
            pending, &writer, &handle, has_code, sink,
        ));
    }

    // ── Phase 2d: HTML post-code/CSS barrier (design D4) ──────────────
    // Code + CSS have joined, so the file set, the css class registry, and the
    // css_id nodes now exist in the building store. Resolve the deferred HTML
    // edges (links, imports, assets, html_id↔css_id correspondence, class usage)
    // against it. Unlike css/markdown this always opens a reader — the HTML and
    // CSS nodes it reads exist regardless of whether any code driver ran.
    if let Some(pending) = html_pending {
        let sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
        let root = runner.workspace.root().to_path_buf();
        reports.push(resolve_html_unit(pending, &writer, &handle, &root, sink));
    }

    // ── Phase 2e: code→table barrier ─────────────────────────────────
    // Code symbols and their body extents exist, and so do the table nodes the
    // `.sql` producer wrote — so the tables a function's own source names can
    // be resolved against the same identities a migration declares. Without
    // this a table is reachable from `.sql` and nothing else, and "what code
    // reads this table" has no answer.
    // Reports only when it had something to scan. `has_code` is not the gate:
    // it says a code driver was configured, not that it produced symbols — an
    // unavailable driver leaves the flag set and the graph empty, and a step
    // that filed a report there would claim work it did not do.
    // Phase 2f rides along: the XML↔SQL bridge needs the same reader and the
    // same table-id allocator, and neither input depends on code.
    let bridge = runner.xml_sql.as_ref();
    if has_code || bridge.is_some() {
        let code_sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
        let xml_sink = BatchSink::new(writer.clone(), handle.clone(), batch_size);
        let root = runner.workspace.root().to_path_buf();
        reports.extend(resolve_table_units(
            &writer, &handle, &root, has_code, bridge, code_sink, xml_sink,
        ));
    }

    // ── Phase 3 + 4: aggregate + finalize ───────────────────────────
    // Both are async; drive them on one plain (runtime-free) thread.
    let t_agg = std::time::Instant::now();
    #[expect(
        clippy::map_err_ignore,
        reason = "a joined thread's panic payload is opaque — WorkerPanicked is the actionable error"
    )]
    let (writer, agg) = std::thread::scope(|scope| {
        scope
            .spawn(
                move || -> Result<(DbWriter, Option<(usize, usize)>), PipelineError> {
                    let agg = handle.block_on(async {
                        let agg = crate::aggregate::compute_and_persist(
                            &writer,
                            &layout,
                            post_aggregate_hook,
                            atlas.as_ref(),
                        )
                        .await?;
                        writer.finalize().await?;
                        Ok::<_, DbError>(agg)
                    })?;
                    Ok((writer, agg))
                },
            )
            .join()
            .map_err(|_| PipelineError::WorkerPanicked)?
    })?;
    let (agg_nodes, agg_edges) = agg.unwrap_or((0, 0));
    progress(ProgressEvent::AggregateComputed {
        nodes: agg_nodes as u64,
        edges: agg_edges as u64,
        elapsed_ms: t_agg.elapsed().as_millis(),
    });
    progress(ProgressEvent::EndRunComplete {
        elapsed_ms: t_agg.elapsed().as_millis(),
    });

    if bench {
        eprintln!(
            "BENCH pipeline: total={}ms aggregate+finalize={}ms",
            t0.elapsed().as_millis(),
            t_agg.elapsed().as_millis(),
        );
    }
    progress(ProgressEvent::Completed {
        total_ms: t0.elapsed().as_millis(),
    });
    Ok((reports, writer))
}

/// Phase-1 preflight: every configured ingester CLI must be available
/// before any store write happens.
fn preflight(runner: &IndexerDriver) -> Result<(), PipelineError> {
    let check = |lang: &str, cmd: PathBuf| -> Result<(), PipelineError> {
        if is_command_available(&cmd) {
            Ok(())
        } else {
            Err(PipelineError::MissingCli(format!(
                "{lang}: required command `{}` not found on PATH",
                cmd.display()
            )))
        }
    };
    let mut any_docker = false;
    for d in &runner.scip_drivers {
        let cmd = d.command();
        any_docker |= cmd.to_str() == Some("docker");
        check(d.language_id(), cmd)?;
    }
    for d in &runner.jsonl_drivers {
        let cmd = d.command();
        any_docker |= cmd.to_str() == Some("docker");
        check(d.language_id(), cmd)?;
    }
    // A docker-runtime language needs a running daemon, not just the `docker`
    // binary on PATH (which the checks above already require).
    if any_docker && !crate::docker::daemon_available() {
        return Err(PipelineError::MissingCli(
            "docker daemon is not responding (`docker info` failed) — is Docker running?".into(),
        ));
    }
    // Create + chown each cache volume so the `--user` indexer containers can
    // write them (a fresh named volume is root-owned).
    for volume in &runner.docker_cache_volumes {
        crate::docker::ensure_cache_volume(volume).map_err(PipelineError::MissingCli)?;
    }
    // Swift alone is provisioned here rather than by the image entrypoint — see
    // the field docs. Failing here is correct: indexing Swift without a
    // toolchain is the silent-zero this whole change removes.
    if let Some((image, version)) = &runner.swift_toolchain {
        crate::docker::provision_swift_from_image(image, version)
            .map_err(PipelineError::MissingCli)?;
    }
    Ok(())
}

/// Whether `cmd` resolves to an existing file — either a path with a
/// directory component (checked directly) or a bare name found on `PATH`.
fn is_command_available(cmd: &Path) -> bool {
    if cmd.components().count() > 1 {
        return cmd.exists();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(cmd).exists())
}

/// Run markdown phase 1 (md↔md nodes + links) as one ingest unit. Returns its
/// report plus the [`MarkdownPending`] to resolve after the code barrier (`None`
/// on failure, so the barrier step is skipped).
///
/// [`MarkdownPending`]: crate::markdown::MarkdownPending
fn markdown_phase1_unit(
    config: &kenn_config::MarkdownConfig,
    root: &Path,
    sink: BatchSink,
) -> (Vec<RunReport>, Option<crate::markdown::MarkdownPending>) {
    let mut report = RunReport::started("markdown", IN_PROCESS_PRODUCER_VERSION, "<corpus>");
    let pending = match crate::markdown::ingest_markdown_phase1(config, root, sink) {
        Ok((counts, pending)) => {
            report.status = RunStatus::Success;
            report.files_seen = counts.files;
            report.symbols_seen = counts.symbols;
            report.defs_seen = counts.defs;
            report.edges_seen = counts.edges;
            Some(pending)
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e.to_string());
            None
        }
    };
    report.finalize();
    (vec![report], pending)
}

/// Run the stylesheet producer (css/sass) as one ingest unit. Returns its
/// report plus the [`CssPending`] (the usage-source files) for the post-code
/// usage pass (`None` on failure, so the barrier step is skipped).
///
/// [`CssPending`]: crate::css::CssPending
fn css_phase1_unit(
    config: &kenn_config::CssConfig,
    root: &Path,
    sink: BatchSink,
) -> (Vec<RunReport>, Option<crate::css::CssPending>) {
    let mut report = RunReport::started("css", IN_PROCESS_PRODUCER_VERSION, "<corpus>");
    let pending = match crate::css::ingest_css_phase1(config, root, sink) {
        Ok((counts, pending)) => {
            report.status = RunStatus::Success;
            report.files_seen = counts.files;
            report.symbols_seen = counts.symbols;
            report.defs_seen = counts.defs;
            report.edges_seen = counts.edges;
            Some(pending)
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e.to_string());
            None
        }
    };
    report.finalize();
    (vec![report], pending)
}

/// Run the HTML producer as one ingest unit. Returns its report plus the
/// [`HtmlPending`] (per-file element lists + id indexes) for the post-code/CSS
/// barrier (`None` on failure, so the barrier step is skipped).
///
/// [`HtmlPending`]: crate::html::HtmlPending
fn html_phase1_unit(
    config: &kenn_config::HtmlConfig,
    root: &Path,
    sink: BatchSink,
) -> (Vec<RunReport>, Option<crate::html::HtmlPending>) {
    let mut report = RunReport::started("html", IN_PROCESS_PRODUCER_VERSION, "<corpus>");
    let pending = match crate::html::ingest_html(config, root, sink) {
        Ok((counts, pending)) => {
            report.status = RunStatus::Success;
            report.files_seen = counts.files;
            report.symbols_seen = counts.symbols;
            report.defs_seen = counts.defs;
            report.edges_seen = counts.edges;
            Some(pending)
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e.to_string());
            None
        }
    };
    report.finalize();
    (vec![report], pending)
}

/// Run the `.sql` producer as one barrier-free ingest unit: discover, parse,
/// resolve, and write in a single pass, leaving no pending state behind. A
/// failure degrades this unit's report and never aborts the run.
fn sql_unit(config: &kenn_config::SqlConfig, root: &Path, sink: BatchSink) -> RunReport {
    let mut report = RunReport::started("sql", IN_PROCESS_PRODUCER_VERSION, "<corpus>");
    match crate::sql::ingest::ingest_sql(config, root, sink) {
        Ok(counts) => {
            report.status = RunStatus::Success;
            report.files_seen = counts.files;
            report.symbols_seen = counts.tables + counts.statements + counts.files;
            report.edges_seen = counts.edges;
            if counts.unreadable > 0 || counts.unparsed > 0 {
                report.warnings.push(format!(
                    "sql: {} unreadable file(s), {} unparsed statement(s)",
                    counts.unreadable, counts.unparsed
                ));
            }
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e.to_string());
        }
    }
    report
}

/// Run the `.xml` producer as one barrier-free ingest unit. A malformed or
/// unreadable document degrades this unit's report and never aborts the run.
fn xml_unit(config: &kenn_config::XmlConfig, root: &Path, sink: BatchSink) -> RunReport {
    let mut report = RunReport::started("xml", IN_PROCESS_PRODUCER_VERSION, "<corpus>");
    match crate::xml::ingest::ingest_xml(config, root, sink) {
        Ok(counts) => {
            report.status = RunStatus::Success;
            report.files_seen = counts.files;
            report.symbols_seen = counts.elements + counts.files;
            report.edges_seen = counts.edges;
            if counts.failed > 0 {
                report
                    .warnings
                    .push(format!("xml: {} unusable document(s)", counts.failed));
            }
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e.to_string());
        }
    }
    report
}

/// Run the text-fallback producer as one barrier-free ingest unit (design D1):
/// discover + split + walk + write happen in a single pass, so there is no
/// pending state to resolve after the code join.
fn text_unit(corpus: &crate::text::TextCorpus, root: &Path, sink: BatchSink) -> Vec<RunReport> {
    let mut report = RunReport::started("text", IN_PROCESS_PRODUCER_VERSION, "<corpus>");
    match crate::text::ingest_text(&corpus.config, root, &corpus.claimed_exts, sink) {
        Ok(counts) => {
            report.status = RunStatus::Success;
            report.files_seen = counts.files;
            report.symbols_seen = counts.symbols;
            report.defs_seen = counts.defs;
            report.edges_seen = counts.edges;
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e.to_string());
        }
    }
    report.finalize();
    vec![report]
}

/// Resolve the deferred HTML edges against the building store (the post-code/CSS
/// barrier, design D4): opens a read snapshot — always, since the HTML/CSS nodes
/// it reads exist regardless of code — and emits the link/import/asset/
/// correspondence/class-usage edges plus their stub nodes. Runs on a plain
/// (runtime-free) thread so the sink/reader `block_on` calls never run on a
/// runtime worker.
fn resolve_html_unit(
    pending: crate::html::HtmlPending,
    writer: &DbWriter,
    handle: &tokio::runtime::Handle,
    root: &Path,
    sink: BatchSink,
) -> RunReport {
    let mut report = RunReport::started("html-resolve", "0", "<corpus>");
    let outcome: Result<crate::html::HtmlResolveCounts, String> = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let reader = handle
                    .block_on(kenn_store::reader_from_writer(writer))
                    .map_err(|e| e.to_string())?;
                crate::html::resolve_html(pending, &reader, handle, root, sink)
                    .map_err(|e| e.to_string())
            })
            .join()
            .unwrap_or_else(|_| Err("html-resolve thread panicked".into()))
    });
    match outcome {
        Ok(counts) => {
            report.status = RunStatus::Success;
            report.symbols_seen = counts.stubs;
            report.edges_seen = counts.edges;
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e);
        }
    }
    report.finalize();
    report
}

/// Resolve the deferred CSS class usages against the building code graph (the
/// post-code barrier). When `has_code`, opens a read snapshot and emits
/// `uses_css_class` edges; a code-less run resolves to nothing. Runs on a plain
/// (runtime-free) thread so the sink/reader `block_on` calls never run on a
/// runtime worker.
fn resolve_css_usage_unit(
    pending: crate::css::CssPending,
    writer: &DbWriter,
    handle: &tokio::runtime::Handle,
    has_code: bool,
    sink: BatchSink,
) -> RunReport {
    let mut report = RunReport::started("css-usage", "0", "<corpus>");
    let outcome: Result<crate::css::CssUsageCounts, String> = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let reader = if has_code {
                    Some(
                        handle
                            .block_on(kenn_store::reader_from_writer(writer))
                            .map_err(|e| e.to_string())?,
                    )
                } else {
                    None
                };
                let code = reader.as_ref().map(|r| (r, handle));
                crate::css::resolve_css_usage(pending, code, sink).map_err(|e| e.to_string())
            })
            .join()
            .unwrap_or_else(|_| Err("css-usage resolver thread panicked".into()))
    });
    match outcome {
        Ok(counts) => {
            report.status = RunStatus::Success;
            report.edges_seen = counts.edges;
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e);
        }
    }
    report.finalize();
    report
}

/// Resolve the deferred in-repo markdown links against the building code graph
/// (the post-code barrier, design D4/6.2). When `has_code`, opens a read
/// snapshot over the writer and resolves md→code edges; otherwise every deferred
/// link dangles. Runs on a plain (runtime-free) thread so the sink's and
/// reader's `block_on` calls never run on a runtime worker.
fn resolve_markdown_code_unit(
    pending: crate::markdown::MarkdownPending,
    writer: &DbWriter,
    handle: &tokio::runtime::Handle,
    has_code: bool,
    workspace_root: &std::path::Path,
    sink: BatchSink,
) -> RunReport {
    let mut report = RunReport::started("markdown-code", "0", "<corpus>");
    let outcome: Result<crate::markdown::MarkdownCounts, String> = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let reader = if has_code {
                    Some(
                        handle
                            .block_on(kenn_store::reader_from_writer(writer))
                            .map_err(|e| e.to_string())?,
                    )
                } else {
                    None
                };
                let code = reader.as_ref().map(|r| (r, handle));
                let exists = crate::markdown::FsPaths { workspace_root };
                crate::markdown::resolve_markdown_code(pending, code, &exists, sink)
                    .map_err(|e| e.to_string())
            })
            .join()
            .unwrap_or_else(|_| Err("markdown-code resolver thread panicked".into()))
    });
    match outcome {
        Ok(counts) => {
            report.status = RunStatus::Success;
            report.symbols_seen = counts.symbols;
            report.edges_seen = counts.edges;
        }
        Err(e) => {
            report.status = RunStatus::Failed;
            report.failed_projects.push(e);
        }
    }
    report.finalize();
    report
}

/// Resolve the tables the workspace's own source names, against the identities
/// the `.sql` producer wrote. Runs on a plain (runtime-free) thread so the
/// sink's and reader's `block_on` calls never run on a runtime worker.
///
/// The minter is built here and would be passed to any other minting barrier
/// step: two steps allocating `Sql` short ids independently from the same
/// high-water mark hand two tables one id.
/// Both table-resolution barrier steps, sharing one reader and one allocator.
///
/// They are one function because the allocator must be one object. Both mint
/// external tables into the `Sql` `ShortId` partition past the `.sql` pass's
/// high-water mark, and two allocators built independently from that mark both
/// compute the same next id — handing two different tables one id, with the
/// loser's edges silently retargeted and nothing downstream reporting it.
///
/// Each step still reports separately, so a failure in one leaves the other's
/// numbers intact.
fn resolve_table_units(
    writer: &DbWriter,
    handle: &tokio::runtime::Handle,
    workspace_root: &Path,
    run_code: bool,
    xml_config: Option<&kenn_config::XmlSqlConfig>,
    code_sink: BatchSink,
    xml_sink: BatchSink,
) -> Vec<RunReport> {
    type Outcome = (
        Option<Result<crate::code_sql::resolve::CodeSqlCounts, String>>,
        Option<Result<crate::xml_sql::resolve::XmlSqlCounts, String>>,
    );
    let outcome: Outcome = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let reader = match handle.block_on(kenn_store::reader_from_writer(writer)) {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = e.to_string();
                        return (
                            run_code.then(|| Err(msg.clone())),
                            xml_config.map(|_| Err(msg)),
                        );
                    }
                };
                let existing = match handle.block_on(kenn_store::api::Reader::scan_symbols(&reader))
                {
                    Ok(rows) => rows.into_iter().map(|s| s.id).collect::<Vec<_>>(),
                    Err(e) => {
                        let msg = e.to_string();
                        return (
                            run_code.then(|| Err(msg.clone())),
                            xml_config.map(|_| Err(msg)),
                        );
                    }
                };
                let mut minter = crate::sql::mint::TableMinter::after_existing(existing);
                let code = run_code.then(|| {
                    crate::code_sql::ingest::ingest_code_tables(
                        &reader,
                        handle,
                        workspace_root,
                        &mut minter,
                        code_sink,
                    )
                    .map_err(|e| e.to_string())
                });
                let xml = xml_config.map(|cfg| {
                    crate::xml_sql::ingest::ingest_xml_tables(
                        &reader,
                        handle,
                        cfg,
                        &mut minter,
                        xml_sink,
                    )
                    .map_err(|e| e.to_string())
                });
                (code, xml)
            })
            .join()
            .unwrap_or_else(|_| {
                let msg = "table resolver thread panicked".to_owned();
                (
                    run_code.then(|| Err(msg.clone())),
                    xml_config.map(|_| Err(msg)),
                )
            })
    });

    let mut reports = Vec::new();
    if let Some(code) = outcome.0 {
        // Reports only when it had something to scan. `has_code` is not the
        // gate: it says a code driver was configured, not that it produced
        // symbols — an unavailable driver leaves the flag set and the graph
        // empty, and a step that filed a report there would claim work it did
        // not do.
        let skip = matches!(&code, Ok(c) if c.bodies_scanned == 0);
        if !skip {
            let mut report = RunReport::started("code-tables", "0", "<corpus>");
            match code {
                Ok(counts) => {
                    report.status = RunStatus::Success;
                    record_code_table_counts(&mut report, &counts);
                }
                Err(e) => {
                    report.status = RunStatus::Failed;
                    report.failed_projects.push(e);
                }
            }
            report.finalize();
            reports.push(report);
        }
    }
    if let Some(xml) = outcome.1 {
        let skip = matches!(&xml, Ok(c) if c.elements_scanned == 0);
        if !skip {
            let mut report = RunReport::started("xml-tables", "0", "<corpus>");
            match xml {
                Ok(counts) => {
                    report.status = RunStatus::Success;
                    record_xml_table_counts(&mut report, &counts);
                }
                Err(e) => {
                    report.status = RunStatus::Failed;
                    report.failed_projects.push(e);
                }
            }
            report.finalize();
            reports.push(report);
        }
    }
    reports
}

/// Put the XML→table pass's counts on its report.
///
/// The two arms are reported separately because they answer different
/// questions: a zero text count means the workspace writes no SQL in element
/// bodies, while a zero attribute count with rules configured means the rules
/// match nothing — a misconfiguration, not an absence.
pub(super) fn record_xml_table_counts(
    report: &mut RunReport,
    counts: &crate::xml_sql::resolve::XmlSqlCounts,
) {
    report.symbols_seen = counts.tables_minted;
    report.edges_seen = counts.refs_emitted;
    report.def_bodies_seen = counts.elements_scanned;
    report.bodies_with_literals = counts.elements_with_sql + counts.elements_with_attribute;
    warn_on_dropped_refs(report, "xml→table", counts.refs_dropped);
}

/// A reference that reached no table node is a defect, not a degradation.
///
/// It means the identity a reference resolved to is not one anything minted,
/// which can only happen if resolution and minting disagree. `emit_table_edges`
/// discards such an edge — the right recovery, and previously a silent one: a
/// mint guard keyed on a table's bare name while references carried the whole
/// key hid a lost `createTable` declaration through a full corpus run, every
/// unit test, and a green gate. Surfacing the count is what turns the next
/// occurrence into a one-line report.
fn warn_on_dropped_refs(report: &mut RunReport, pass: &str, dropped: u64) {
    if dropped > 0 {
        report.warnings.push(format!(
            "{pass}: {dropped} reference(s) reached no table node and were dropped \
             — resolution and minting disagree on an identity"
        ));
    }
}

/// Put the code→table pass's counts on its report.
///
/// Three numbers, not one, because an ORM-mapped codebase is fully
/// instrumented against its database and yields no edges here. A bare zero
/// would read as "no code touches this"; alongside bodies scanned and bodies
/// carrying literals it reads as "the SQL is not written as a literal", which
/// is the true answer.
///
/// `files_seen` deliberately stays zero: it rolls up into the per-language file
/// total `kenn index` prints, and this step re-reads files another unit already
/// counted, so borrowing it would double them.
pub(super) fn record_code_table_counts(
    report: &mut RunReport,
    counts: &crate::code_sql::resolve::CodeSqlCounts,
) {
    report.symbols_seen = counts.tables_minted;
    report.edges_seen = counts.refs_emitted;
    report.def_bodies_seen = counts.bodies_scanned;
    report.bodies_with_literals = counts.bodies_with_literals;
    warn_on_dropped_refs(report, "code→table", counts.refs_dropped);
}

/// A report standing in for an ingester thread that panicked.
fn panicked_report() -> RunReport {
    let mut r = RunReport::started("?", "?", "<panicked>");
    r.status = RunStatus::Failed;
    r.failed_projects.push("ingester thread panicked".into());
    r.finalize();
    r
}
