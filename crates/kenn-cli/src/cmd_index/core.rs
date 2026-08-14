use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use kenn_config::Config;
use kenn_indexer::report::{aggregate_status, all_documents_outside_root, RunReport, RunStatus};
use kenn_indexer::{
    aggregate_counts, build_snapshot_meta, build_workspace, configure_runner,
    persist_run_artifacts, SnapshotCounts, SnapshotMeta, SNAPSHOT_META_FILE,
};
use kenn_store::{
    compute_diff, compute_staleness_key, lifecycle, open_writer, Layout, MetricSnapshot,
    StalenessKey, Store, WriterOptions,
};

use crate::exit::ExitCodes;

/// Build the [`AtlasContext`](kenn_indexer::atlas::producer::AtlasContext) for a
/// run: the output dir (the run's `atlas/`, carried on publish) plus header facts
/// — workspace name (source-root leaf), freshness (short HEAD), and a timestamp
/// (the run id, already ISO-8601).
fn atlas_context(
    source_root: &Path,
    run_dir: &Path,
    committed_root: &Path,
) -> kenn_indexer::atlas::producer::AtlasContext {
    let leaf = |p: &Path, dflt: &str| {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(dflt)
            .to_string()
    };
    let freshness = std::process::Command::new("git")
        .arg("-C")
        .arg(source_root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(
            || "uncommitted".to_string(),
            |s| format!("HEAD {}", s.trim()),
        );
    kenn_indexer::atlas::producer::AtlasContext {
        out_dir: run_dir.join("atlas"),
        source_root: source_root.to_path_buf(),
        pointer_dir: Some(committed_root.to_path_buf()),
        workspace_name: leaf(source_root, "workspace"),
        freshness,
        timestamp: leaf(run_dir, "unknown"),
    }
}

pub fn run(
    layout: Layout,
    config: Config,
    force: bool,
    json: bool,
    repack: bool,
) -> Result<ExitCodes> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_async(layout, config, force, json, repack))
}

/// True when `kenn index` should short-circuit because the workspace
/// is unchanged since the last published snapshot. Honors a `Skip`
/// only with git-aware skipping enabled — non-git workspaces with the
/// flag off can't trust `decide_startup_state`'s fallback. Pure
/// function except for the inner staleness probe (`git status` /
/// tree-fingerprint walk), so the precedence branches are testable.
pub fn should_skip_for_staleness(
    force: bool,
    store: &Store,
    source_root: &Path,
    git_aware: bool,
    config_sig: u64,
) -> bool {
    if force || !git_aware {
        return false;
    }
    matches!(
        kenn_store::decide_startup_state(store, source_root, git_aware, config_sig),
        kenn_store::StartupDecision::Skip { .. }
    )
}

/// True when `kenn.toml` enables at least one indexable language. Kept as
/// a standalone helper so the per-language branch count lives here rather
/// than inflating `run_async`'s cyclomatic complexity.
fn any_language_enabled(config: &Config) -> bool {
    let l = &config.language;
    l.csharp.enabled
        || l.rust.enabled
        || l.typescript.enabled
        || l.python.enabled
        || l.go.enabled
        || l.markdown.enabled
        || l.css.enabled
        || l.html.enabled
        || l.swift.enabled
        || l.sql.enabled
        || l.xml.enabled
}

#[expect(
    clippy::too_many_lines,
    reason = "linear orchestration with explicit BENCH instrumentation; splitting hurts readability"
)]
async fn run_async(
    layout: Layout,
    config: Config,
    force: bool,
    json: bool,
    repack: bool,
) -> Result<ExitCodes> {
    let source_root = layout.source_root();
    let store = Store::open(layout.clone())?;
    if let Err(e) = lifecycle::recover(&store) {
        return Err(anyhow::anyhow!("recover incomplete runs: {e}"));
    }

    let config_sig = config.indexing_signature();
    let staleness = if config.staleness.git_aware_skip {
        compute_staleness_key(source_root, config_sig)
    } else {
        StalenessKey::Unknown
    };

    if should_skip_for_staleness(
        force,
        &store,
        source_root,
        config.staleness.git_aware_skip,
        config_sig,
    ) {
        emit_progress(json, "skipped", "staleness key unchanged since last run");
        return Ok(ExitCodes::Ok);
    }

    if !any_language_enabled(&config) {
        emit_progress(
            json,
            "no-languages",
            "no languages enabled; nothing to index — see kenn.toml",
        );
    }

    emit_progress(
        json,
        "begin",
        &format!("indexing {}", source_root.display()),
    );
    let handle = match lifecycle::begin_indexing(&store) {
        Ok(h) => h,
        Err(lifecycle::BeginError::LockHeld(_)) => {
            eprintln!("error: another `kenn index` is already running on this workspace");
            return Ok(ExitCodes::LockHeld);
        }
        Err(e) => return Err(e.into()),
    };
    // Workspace attached to the run dir so SCIP / JSONL drivers emit
    // per-run intermediates (§5.3 / §5.4).
    let ws = build_workspace(source_root, &config)?
        .with_layout(layout.clone())
        .with_run_dir(handle.run_dir().to_path_buf());

    let runner = configure_runner(ws, &config);

    let bench_t_sink_open = std::time::Instant::now();
    // Reconcile against the committed vector sidecar — always in-repo
    // under the layout's committed root.
    let vectors_model_id = kenn_store::current_model_id();
    let sink = open_writer(
        handle.run_dir(),
        WriterOptions {
            vectors_dir: Some(kenn_store::code_generation_dir(&layout, &vectors_model_id)),
            vectors_legacy_dir: Some(layout.code_vectors_dir()),
            vectors_model_id: Some(vectors_model_id),
            ..WriterOptions::default()
        },
    )
    .await
    .context("opening writer at runs/{id}/")?;
    bench_log("sink_open", bench_t_sink_open);
    let bench_t_pipeline = std::time::Instant::now();
    let batch_size = config.ingest.batch_size;
    let hook = kenn_analyze::analysis_hook_from_config(&config);
    let atlas_ctx = atlas_context(source_root, handle.run_dir(), layout.committed_root());
    let outcome = tokio::task::spawn_blocking(move || {
        kenn_indexer::run_pipeline_with_progress(
            &runner,
            sink,
            batch_size,
            |_| {},
            hook,
            Some(atlas_ctx),
        )
    })
    .await?;
    bench_log("run_pipeline_total", bench_t_pipeline);

    let (reports, _sink) = match outcome {
        Ok(p) => p,
        Err(e) => {
            drop(handle.abort());
            eprintln!("error: indexer failed: {e}");
            return Ok(ExitCodes::IndexerFailed);
        }
    };

    let aggregate_status = aggregate_status(&reports);
    // index-run-reporting spec: degraded runs name each affected language
    // on stderr at index time, not only in `kenn status` afterwards.
    let rollups = rollup_by_language(&reports);
    for line in run_warning_lines(&reports, &rollups) {
        eprintln!("{line}");
    }
    if matches!(aggregate_status, RunStatus::Failed) {
        drop(handle.abort());
        eprintln!("error: every indexer reported Failed");
        return Ok(ExitCodes::IndexerFailed);
    }
    // Tripwire: documents were dropped outside the root AND none survived, so the
    // snapshot would be empty. Abort with a non-zero exit rather than publish it
    // (today's silent bug). Checked after the Failed guard so a genuinely failed
    // run reports the failure; a partial drop — some in-root docs survived — is a
    // warning above, not a failure.
    if let Some(dropped) = all_documents_outside_root(&reports) {
        drop(handle.abort());
        eprintln!(
            "error: all {dropped} document(s) fell outside the workspace root — \
             the index would be empty (the indexer's project root does not match \
             the workspace root)"
        );
        return Ok(ExitCodes::IndexerFailed);
    }

    let counts = aggregate_counts(&reports);
    let prev_meta = load_live_meta(&store);
    let regressions = compute_regressions(prev_meta.as_ref(), &counts, &reports, &config);

    let bench_t_persist = std::time::Instant::now();
    // The run id is the lifecycle handle's run dir leaf — ISO-8601
    // timestamp emitted by `Layout::new_run_id` (D1, §1.12).
    let run_id = handle
        .run_dir()
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let meta = build_snapshot_meta(
        &run_id,
        aggregate_status,
        &counts,
        &reports,
        regressions,
        &staleness,
        source_root,
    );
    persist_run_artifacts(handle.run_dir(), &meta, &reports)?;
    bench_log("persist", bench_t_persist);

    let bench_t_publish = std::time::Instant::now();
    // Stage this run's findings mirror from the committed records and
    // hold the findings-publish lock across the live flip (§B BUILD
    // PATH), so a finding committed concurrently isn't dropped.
    let findings_lock = kenn_store::stage_findings_for_publish(&layout, handle.run_dir())
        .await
        .context("staging findings for publish")?;
    let snap_path = handle.publish()?;
    drop(findings_lock);
    bench_log("publish", bench_t_publish);
    emit_progress(
        json,
        "published",
        &format!("live → {}", snap_path.display()),
    );

    // Atlas handle (`atlas` capability): announce the just-published run's
    // atlas `index.md` on a marked, greppable line — human mode only, so a
    // `--json` run's stream isn't corrupted (the JSON field is a fast-follow).
    // `live` is a pointer file now (no traversable symlink to hang a stable
    // `.kenn/atlas` off), so the path names the published run directly.
    let atlas_index = snap_path.join("atlas").join("index.md");
    if atlas_index.exists() && !json {
        println!("atlas: {}", atlas_index.display());
    }

    // Capability probe: a Rust unit that produced definitions but zero body
    // extents means the resolved rust-analyzer emits no SCIP `enclosing_range`
    // (a pre-Dec-2025 build). `get_source` then returns declaration lines
    // instead of whole items. Warn once, pointing at the upgrade.
    if reports
        .iter()
        .any(|r| r.indexer_name == "rust-analyzer" && r.defs_seen > 0 && r.def_bodies_seen == 0)
    {
        eprintln!(
            "warning: rust-analyzer emitted no enclosing_range — `get source` \
             returns declaration lines only for Rust. Upgrade to a Dec-2025+ \
             build (`brew install rust-analyzer` or `rustup update`)."
        );
    }

    if repack {
        repack_vector_dirs(&layout, json);
    }

    let bench_t_gc = std::time::Instant::now();
    if let Err(e) = lifecycle::gc(&store, config.lifecycle.gc_keep) {
        eprintln!("warning: GC failed: {e}");
    }
    bench_log("gc", bench_t_gc);

    Ok(ExitCodes::Ok)
}

/// `KENN_BENCH` is read once at first use and cached — env-var lookups
/// hit a process-global mutex on macOS, so checking it at every
/// checkpoint in the hot path is real overhead. The env is set before
/// `kenn index` starts; mid-run mutation would not be honoured anyway.
static BENCH_ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var_os("KENN_BENCH").is_some());

/// Emit a `BENCH` timing line for `label` when `KENN_BENCH` is set.
/// Centralised here so the orchestrator `run_async` doesn't sprout
/// `if bench { eprintln!(...) }` branches at every checkpoint.
fn bench_log(label: &str, since: std::time::Instant) {
    if *BENCH_ENABLED {
        eprintln!("BENCH cmd_index: {label}={}ms", since.elapsed().as_millis());
    }
}

/// Promote dev-local `seg-*.bin` files to canonical `pack-*.bin` in every
/// sidecar directory — each generation dir plus the legacy flat dirs (D13). The content hash is preserved — the rename
/// is a directory-entry flip, not a content rewrite. Idempotent (no-op
/// when no segs exist, which is the common case in CI invocations).
fn repack_vector_dirs(layout: &Layout, json: bool) {
    for vectors_dir in kenn_store::sidecar_dirs(layout.vectors_root()) {
        match kenn_store::promote_segs_to_packs(&vectors_dir) {
            Ok(promoted) if !promoted.is_empty() => emit_progress(
                json,
                "repack",
                &format!(
                    "promoted {} seg-* → pack-* in {}",
                    promoted.len(),
                    vectors_dir.display()
                ),
            ),
            Ok(_) => {}
            Err(e) => eprintln!(
                "warning: --repack promote failed in {}: {e}",
                vectors_dir.display()
            ),
        }
    }
}

fn compute_regressions(
    prev: Option<&SnapshotMeta>,
    counts: &SnapshotCounts,
    reports: &[RunReport],
    config: &Config,
) -> Vec<kenn_store::RegressionWarning> {
    let Some(prev) = prev else { return Vec::new() };
    compute_diff(
        &MetricSnapshot {
            documents: prev.documents,
            symbols: prev.symbols,
            definitions: prev.definitions,
            edges: prev.edges,
            failed_projects: prev.failed_projects.len() as u64 + prev.failed_overflow,
        },
        &MetricSnapshot {
            documents: counts.documents,
            symbols: counts.symbols,
            definitions: counts.definitions,
            edges: counts.edges,
            failed_projects: aggregate_failed(reports),
        },
        config.metrics.regression_threshold_pct,
    )
}

fn load_live_meta(store: &Store) -> Option<SnapshotMeta> {
    let live = store.live_target()?;
    let path = live.join(SNAPSHOT_META_FILE);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// One language's reports rolled up for the post-run stderr summary.
struct LanguageRollup<'a> {
    /// Display label and grouping key: the report's language db name, or
    /// the raw producer name for language-less units (`html-resolve`).
    label: &'a str,
    /// The rollup's language when its reports carry one — sources the
    /// claimed extensions for the zero-files warning.
    language: Option<kenn_model::Language>,
    worst: RunStatus,
    files: u64,
    first_failure: Option<&'a str>,
    failures: u64,
    first_warning: Option<&'a str>,
    warnings: u64,
}

/// Grouping/display key for one report: reports carry their language
/// (drivers state it at construction), so a language's branded per-unit
/// reports ("rust-analyzer") and its language-id failure reports ("rust")
/// collapse into one rollup.
fn rollup_label(r: &RunReport) -> &str {
    match r.language {
        Some(l) => l.db_name(),
        None => r.indexer_name.as_str(),
    }
}

/// Group per-unit reports by language (falling back to producer name),
/// preserving first-seen order, rolling up worst status, file totals, and
/// failure attributions (including each report's structured overflow).
fn rollup_by_language(reports: &[RunReport]) -> Vec<LanguageRollup<'_>> {
    let mut rollups: Vec<LanguageRollup> = Vec::new();
    for r in reports {
        let label = rollup_label(r);
        let pos = rollups
            .iter()
            .position(|e| e.label == label)
            .unwrap_or_else(|| {
                rollups.push(LanguageRollup {
                    label,
                    language: r.language,
                    worst: RunStatus::Success,
                    files: 0,
                    first_failure: None,
                    failures: 0,
                    first_warning: None,
                    warnings: 0,
                });
                rollups.len() - 1
            });
        let Some(entry) = rollups.get_mut(pos) else {
            continue; // unreachable: pos comes from position() or the push above
        };
        entry.worst = entry.worst.max(r.status);
        entry.files += r.files_seen;
        entry.failures += r.failed_projects.len() as u64 + r.failed_overflow;
        if entry.first_failure.is_none() {
            entry.first_failure = r.failed_projects.first().map(String::as_str);
        }
        entry.warnings += r.warnings.len() as u64 + r.warnings_overflow;
        if entry.first_warning.is_none() {
            entry.first_warning = r.warnings.first().map(String::as_str);
        }
    }
    rollups
}

/// One stderr line per language whose reports carry producer warnings —
/// independent of status, because the warnings exist precisely for
/// degradations that keep the unit `Success` (e.g. stale index-store
/// units kept on a trusted read).
fn producer_warning_lines(rollups: &[LanguageRollup<'_>]) -> Vec<String> {
    rollups
        .iter()
        .filter(|e| e.warnings > 0)
        .filter_map(|e| {
            let first_line = e.first_warning?.lines().next()?;
            let more = if e.warnings > 1 {
                format!(" (+{} more)", e.warnings - 1)
            } else {
                String::new()
            };
            Some(format!("warning: {}: {first_line}{more}", e.label))
        })
        .collect()
}

/// One stderr line per language whose reports contain any non-`Success`
/// status: `warning: <lang>: <status> — <first failure> (+N more)`.
/// Empty for a clean run, so the happy path stays quiet.
fn degraded_language_lines(rollups: &[LanguageRollup<'_>]) -> Vec<String> {
    rollups
        .iter()
        .filter(|e| !matches!(e.worst, RunStatus::Success))
        .map(|e| {
            let status = match e.worst {
                RunStatus::Failed => "failed",
                _ => "partial",
            };
            let label = e.label;
            let Some(f) = e.first_failure else {
                return format!("warning: {label}: {status}");
            };
            // Failure messages can embed a multi-line stderr tail; the
            // summary keeps one line per language.
            let first_line = f.lines().next().unwrap_or(f);
            let more = if e.failures > 1 {
                format!(" (+{} more)", e.failures - 1)
            } else {
                String::new()
            };
            format!("warning: {label}: {status} — {first_line}{more}")
        })
        .collect()
}

/// Warn when a degraded language indexed zero files: its extensions are
/// claimed (the text fallback skips them), so those files are absent from
/// the snapshot entirely. Requires a non-`Success` report so an enabled
/// language with genuinely no sources (JSONL producers always report once
/// per run) stays quiet. The extension list comes from the same association
/// the text fallback uses, so the warning names exactly the skipped files.
fn zero_file_warnings(rollups: &[LanguageRollup<'_>]) -> Vec<String> {
    rollups
        .iter()
        .filter(|e| e.files == 0 && !matches!(e.worst, RunStatus::Success))
        .filter_map(|e| {
            let exts = kenn_indexer::language_claimed_extensions(e.language?);
            if exts.is_empty() {
                // The text fallback owns no fixed extensions.
                return None;
            }
            let dotted: Vec<String> = exts.iter().map(|x| format!(".{x}")).collect();
            Some(format!(
                "warning: {} indexed 0 files — {} files are absent from the snapshot \
                 (claimed extensions are skipped by the text fallback)",
                e.label,
                dotted.join("/"),
            ))
        })
        .collect()
}

/// All at-index-time warning lines, in display order: degraded languages,
/// producer diagnostics, zero-file degrades, then out-of-root drops. Collapsing
/// the four sources into one list keeps `run_async` a single print loop.
fn run_warning_lines(reports: &[RunReport], rollups: &[LanguageRollup<'_>]) -> Vec<String> {
    let mut lines = degraded_language_lines(rollups);
    lines.extend(producer_warning_lines(rollups));
    lines.extend(zero_file_warnings(rollups));
    lines.extend(out_of_root_warnings(reports));
    lines
}

/// At-index-time warning for each unit that dropped documents outside the
/// workspace root. Mirrors `zero_file_warnings`; the detailed `project_root` vs
/// root mismatch is in each report's persisted `warnings` (shown by `kenn
/// status`).
fn out_of_root_warnings(reports: &[RunReport]) -> Vec<String> {
    reports
        .iter()
        .filter(|r| r.out_of_root_seen > 0)
        .map(|r| {
            format!(
                "warning: {}: {} document(s) fell outside the workspace root",
                rollup_label(r),
                r.out_of_root_seen
            )
        })
        .collect()
}

fn aggregate_failed(reports: &[RunReport]) -> u64 {
    reports
        .iter()
        .filter(|r| matches!(r.status, RunStatus::Failed))
        .count() as u64
        + reports
            .iter()
            .map(|r| r.failed_projects.len() as u64 + r.failed_overflow)
            .sum::<u64>()
}

fn emit_progress(json: bool, kind: &str, message: &str) {
    if json {
        let line = serde_json::json!({"event": kind, "message": message});
        println!("{line}");
    } else {
        println!("{message}");
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
