//! End-to-end indexing workflow used by both `kenn index` (CLI) and
//! `kenn mcp` (server). Combines `Config` parsing, `Store`/`Workspace`
//! setup, lifecycle handling (`begin_indexing` / `publish` / `gc`), and
//! `run_pipeline_with_progress` into a single call.
//!
//! The CLI's `cmd_index` adds regression-warning logic and JSON
//! progress output on top of this; MCP's startup orchestration calls
//! this directly when the snapshot is missing or stale.
//!
//! Lives in `kenn-indexer` (rather than `kenn-store` where it used to
//! live) because it orchestrates the indexer pipeline and the storage
//! layer together. Putting it here keeps `kenn-store` free of any
//! dependency on `kenn-indexer`, which lets the writer/reader trait
//! abstraction live in `kenn-store::api` without a circular dep.
//! See openspec/changes/storage-abstraction.

use std::path::{Path, PathBuf};

use kenn_model::Language;
use kenn_store::layout::StoreError;
use kenn_store::staleness::{compute_staleness_key, StalenessKey};
use kenn_store::{
    lifecycle, open_writer, BeginError, Layout, PublishError, RecoveryError, Store, WriterOptions,
};
use thiserror::Error;

use crate::driver::{
    IndexerDriver, KennDotnet, KennSwift, KennTs, RustAnalyzer, ScipGo, ScipPython,
};
use crate::pipeline::{run_pipeline_with_progress, PostAggregateHook, ProgressEvent};
use crate::report::{aggregate_status, RunReport, RunStatus};
use crate::snapshot::{
    aggregate_counts, build_snapshot_meta, persist_run_artifacts, SnapshotCounts,
};
use crate::{CanonicalizeError, Workspace};
use kenn_config::Config;

/// Errors that can happen while running [`index_workspace`].
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("config: {0}")]
    Config(String),
    #[error("workspace: {0}")]
    Workspace(String),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("recover: {0}")]
    Recover(#[from] RecoveryError),
    #[error("begin: {0}")]
    Begin(#[from] BeginError),
    #[error("sink: {0}")]
    Sink(String),
    #[error("pipeline: {0}")]
    Pipeline(String),
    #[error("publish: {0}")]
    Publish(#[from] PublishError),
    #[error("findings: {0}")]
    Findings(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("join: {0}")]
    Join(String),
    #[error("indexer reported only failures (no successful units)")]
    AllUnitsFailed,
    #[error("every indexed document fell outside the workspace root (empty index)")]
    AllDocumentsOutsideRoot,
}

/// Outcome of a successful workflow run.
pub struct WorkflowOutcome {
    /// Path to the newly-published snapshot inside `<store>/snapshots/`.
    pub snapshot_path: PathBuf,
    /// Per-unit reports collected from the pipeline.
    pub reports: Vec<RunReport>,
    /// Aggregated counts written into the snapshot's `meta.json`.
    pub counts: SnapshotCounts,
}

/// Run a full indexing job end-to-end.
///
/// `progress` receives every pipeline `ProgressEvent`. CLI callers can
/// pass a no-op closure; MCP wires it to its notification pump.
///
/// The CPU-bound pipeline body runs on a `tokio::task::spawn_blocking`
/// thread; the rest of the workflow is async I/O.
#[expect(
    clippy::too_many_lines,
    reason = "linear open → begin → pipeline → tripwires → publish orchestration; splitting scatters the handle/store state"
)]
pub async fn index_workspace<F>(
    layout: &Layout,
    config: &Config,
    progress: F,
    post_aggregate_hook: PostAggregateHook,
) -> Result<WorkflowOutcome, WorkflowError>
where
    F: Fn(ProgressEvent) + Send + Sync + 'static,
{
    let store = Store::open(layout.clone())?;
    lifecycle::recover(&store)?;

    let staleness = if config.staleness.git_aware_skip {
        compute_staleness_key(layout.source_root(), config.indexing_signature())
    } else {
        StalenessKey::Unknown
    };

    let handle = lifecycle::begin_indexing(&store)?;
    // Workspace is attached to the run dir so SCIP / JSONL drivers
    // emit per-run intermediates (§5.3 / §5.4); they're carried with
    // the run on publish.
    let ws = build_workspace(layout.source_root(), config)
        .map_err(|e| WorkflowError::Workspace(e.to_string()))?
        .with_layout(layout.clone())
        .with_run_dir(handle.run_dir().to_path_buf());

    let runner = configure_runner(ws, config);

    // Pass the committed code vector sidecar so `finalize` reconciles cached
    // vectors into `vec0` (no embedding model needed). The generation dir is
    // keyed by the configured model; the legacy flat dir keeps serving
    // pre-generation committed packs.
    let vectors_model_id = kenn_store::current_model_id();
    let sink = open_writer(
        handle.run_dir(),
        WriterOptions {
            vectors_dir: Some(kenn_store::code_generation_dir(layout, &vectors_model_id)),
            vectors_legacy_dir: Some(layout.code_vectors_dir()),
            vectors_model_id: Some(vectors_model_id),
            ..WriterOptions::default()
        },
    )
    .await
    .map_err(|e| WorkflowError::Sink(format!("opening writer: {e}")))?;

    let batch_size = config.ingest.batch_size;
    let atlas_ctx = crate::atlas::producer::AtlasContext {
        out_dir: handle.run_dir().join("atlas"),
        source_root: layout.source_root().to_path_buf(),
        pointer_dir: Some(layout.committed_root().to_path_buf()),
        workspace_name: layout
            .source_root()
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string(),
        freshness: "reindex".to_string(),
        timestamp: handle
            .run_dir()
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string(),
    };
    let pipeline_outcome = tokio::task::spawn_blocking(move || {
        run_pipeline_with_progress(
            &runner,
            sink,
            batch_size,
            progress,
            post_aggregate_hook,
            Some(atlas_ctx),
        )
    })
    .await
    .map_err(|e| WorkflowError::Join(e.to_string()))?;

    let (reports, _sink) = match pipeline_outcome {
        Ok(p) => p,
        Err(e) => {
            drop(handle.abort());
            return Err(WorkflowError::Pipeline(e.to_string()));
        }
    };

    if !reports.is_empty()
        && reports
            .iter()
            .all(|r| matches!(r.status, RunStatus::Failed))
    {
        drop(handle.abort());
        return Err(WorkflowError::AllUnitsFailed);
    }

    // Same tripwire as `kenn index`: documents were dropped outside the root and
    // none survived, so publishing would leave an empty index. Abort rather than
    // publish it. The abort discards the run dir, so the drops are surfaced only
    // by this error (the caller reports it); the per-document `warnings` are
    // persisted to report.json only on the published/partial path below.
    if crate::report::all_documents_outside_root(&reports).is_some() {
        drop(handle.abort());
        return Err(WorkflowError::AllDocumentsOutsideRoot);
    }

    let counts = aggregate_counts(&reports);
    // The run id is the run dir's leaf (an ISO-8601 timestamp). Regressions are
    // CLI-only (they need the previous snapshot's counts), so none are recorded
    // here — the metadata is otherwise identical to a `kenn index` run.
    let run_id = handle
        .run_dir()
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let meta = build_snapshot_meta(
        run_id,
        aggregate_status(&reports),
        &counts,
        &reports,
        Vec::new(),
        &staleness,
        layout.source_root(),
    );
    persist_run_artifacts(handle.run_dir(), &meta, &reports)?;

    // Stage this run's findings mirror from the committed records and
    // hold the findings-publish lock across the live flip (§B BUILD
    // PATH), so a finding committed concurrently isn't dropped.
    let findings_lock = kenn_store::stage_findings_for_publish(layout, handle.run_dir())
        .await
        .map_err(|e| WorkflowError::Findings(e.to_string()))?;
    let snapshot_path = handle.publish()?;
    drop(findings_lock);

    if let Err(e) = lifecycle::gc(&store, config.lifecycle.gc_keep) {
        tracing::warn!(error = %e, "gc after publish failed");
    }

    Ok(WorkflowOutcome {
        snapshot_path,
        reports,
        counts,
    })
}

/// Build the base `Workspace` from the resolved config: workspace excludes,
/// test globs, and per-language excludes. Single source of truth for workspace
/// construction — both the CLI (`kenn index`) and the workflow / MCP
/// `index_workspace` path call this, so no entry path can silently omit a
/// language's excludes. Callers chain `.with_layout(...)` and
/// `.with_run_dir(...)` afterward.
pub fn build_workspace(
    source_root: &Path,
    config: &Config,
) -> Result<Workspace, CanonicalizeError> {
    Workspace::new(source_root, &config.workspace.excludes)?
        .with_test_globs(&config.tests.paths)?
        .with_language_excludes(Language::Rust, &config.language.rust.excludes)?
        .with_language_excludes(Language::TypeScript, &config.language.typescript.excludes)?
        .with_language_excludes(Language::Csharp, &config.language.csharp.excludes)?
        .with_language_excludes(Language::Python, &config.language.python.excludes)?
        .with_language_excludes(Language::Go, &config.language.go.excludes)?
        .with_language_excludes(Language::Swift, &config.language.swift.excludes)
}

/// Build the indexer driver for `ws`, registering one language driver
/// per language enabled in `config`. Single source of truth for producer
/// registration — both the CLI (`kenn index`) and the workflow / MCP
/// `index_workspace` path call this, so no entry path can silently omit a
/// producer.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "linear per-language producer registration table; splitting the arms hurts readability"
)]
pub fn configure_runner(ws: Workspace, config: &Config) -> IndexerDriver {
    let ws_root = ws.root().to_path_buf();
    let mut runner = IndexerDriver::new(ws);
    let dcfg = &config.docker;
    // Dependency-source cache volume. Default (`cache_volume` unset): a
    // per-repository volume bound to the repo's main worktree — shared by all its
    // worktrees, reclaimable by `kenn docker-cache --orphans` when the repo is
    // deleted. Configured `cache_volume = "name"`: one shared cross-repo volume,
    // bound to no directory (never orphaned).
    let (deps_volume, deps_bound_dir) = if let Some(name) = &dcfg.cache_volume {
        (name.clone(), None)
    } else {
        let main = kenn_store::git::main_worktree(&ws_root).unwrap_or_else(|| ws_root.clone());
        (crate::docker::deps_volume_name(&main), Some(main))
    };
    // Per-workspace build-cache volume, only when the user opts into persisting
    // build artifacts; otherwise builds are ephemeral (dropped on `--rm`).
    let build_volume = dcfg
        .persist_build_cache
        .then(|| crate::docker::build_volume_name(&ws_root));
    // Rewrite a language's launcher into a `docker run` wrapper when it opts into
    // `runtime = "docker"`; a no-op for the default local runtime. `source` is the
    // dependency-source cache; `build` is the build-artifact cache.
    let launch = |command: &[String],
                  runtime: kenn_config::Runtime,
                  image: &Option<String>,
                  source: Option<(&'static str, &'static str)>,
                  build: Option<(&'static str, &'static str)>|
     -> Vec<String> {
        // Built when EITHER half is requested — `source.map(…)` would drop a
        // build-only language's cache (Swift) on the floor.
        let cache = (source.is_some() || build.is_some()).then(|| crate::docker::LangCache {
            source: source.map(|(env, subdir)| crate::docker::SourceCache {
                env,
                subdir,
                volume: &deps_volume,
            }),
            build: build.map(|(env, subdir)| crate::docker::BuildCache {
                env,
                subdir,
                volume: build_volume.as_deref(),
            }),
        });
        crate::docker::maybe_docker_command(command, runtime, image.as_deref(), cache, &ws_root)
    };
    if config.language.csharp.enabled {
        runner = runner.with_jsonl_driver(KennDotnet {
            command: launch(
                &config.language.csharp.command,
                config.language.csharp.runtime,
                &config.language.csharp.image,
                Some(("NUGET_PACKAGES", "nuget")),
                None,
            ),
            projects: config.language.csharp.projects.clone(),
            // Restore unless the user opts out: an unrestored project binds no
            // NuGet type, silently (exit 0, symbols, no diagnostic). The docker
            // runtime always starts from an unrestored container.
            skip_restore: !config.language.csharp.restore,
            test_globs: config.tests.paths.clone(),
            test_assembly_regexes: config.tests.assembly_regex.clone(),
            provision_sdk: config.language.csharp.provision_sdk,
            mount: crate::docker::container_mount(config.language.csharp.runtime, &ws_root),
        });
    }
    if config.language.rust.enabled {
        runner = runner.with_scip_driver(RustAnalyzer {
            command: launch(
                &config.language.rust.command,
                config.language.rust.runtime,
                &config.language.rust.image,
                Some(("CARGO_HOME", "cargo")),
                Some(("CARGO_TARGET_DIR", "cargo")),
            ),
            exclude_vendored_libraries: config.language.rust.exclude_vendored_libraries,
            max_threads: config.language.rust.max_threads,
            low_priority: config.language.rust.low_priority,
            mount: crate::docker::container_mount(config.language.rust.runtime, &ws_root),
        });
    }
    if config.language.typescript.enabled {
        runner = runner.with_jsonl_driver(KennTs {
            command: launch(
                &config.language.typescript.command,
                config.language.typescript.runtime,
                &config.language.typescript.image,
                Some(("npm_config_cache", "npm")),
                None,
            ),
            projects: config.language.typescript.projects.clone(),
            mount: crate::docker::container_mount(config.language.typescript.runtime, &ws_root),
        });
    }
    if config.language.python.enabled {
        runner = runner.with_scip_driver(ScipPython {
            command: launch(
                &config.language.python.command,
                config.language.python.runtime,
                &config.language.python.image,
                Some(("PIP_CACHE_DIR", "pip")),
                None,
            ),
            project_name: config.language.python.project_name.clone(),
            project_version: config.language.python.project_version.clone(),
            targets: config.language.python.targets.clone(),
            mount: crate::docker::container_mount(config.language.python.runtime, &ws_root),
        });
    }
    if config.language.go.enabled {
        runner = runner.with_scip_driver(ScipGo {
            command: launch(
                &config.language.go.command,
                config.language.go.runtime,
                &config.language.go.image,
                Some(("GOMODCACHE", "go")),
                Some(("GOCACHE", "go")),
            ),
            mount: crate::docker::container_mount(config.language.go.runtime, &ws_root),
        });
    }
    if config.language.swift.enabled {
        runner = runner.with_jsonl_driver(KennSwift {
            command: launch(
                &config.language.swift.command,
                config.language.swift.runtime,
                &config.language.swift.image,
                None,
                // Docker only: redirect SwiftPM's `.build` (incl. dep checkouts)
                // onto the per-worktree build-cache volume, off the slow host bind
                // mount. kenn-swift reads KENN_SWIFT_SCRATCH; SwiftPM has no scratch
                // env var. Native never sets it — the macOS toolchain breaks on
                // --scratch-path + prepare-for-indexing (see Provisioning.swift).
                Some(("KENN_SWIFT_SCRATCH", "swift")),
            ),
            skip_build: config.language.swift.skip_build,
            projects: config.language.swift.projects.clone(),
            platform: config.language.swift.platform.clone(),
            mount: crate::docker::container_mount(config.language.swift.runtime, &ws_root),
        });
    }
    if config.language.markdown.enabled {
        runner = runner.with_markdown(markdown_with_inherited_excludes(config, &ws_root));
    }
    if config.language.css.enabled {
        runner = runner.with_css(config.language.css.clone());
    }
    if config.language.html.enabled {
        runner = runner.with_html(config.language.html.clone());
    }
    if config.language.text.enabled {
        runner = runner.with_text(config.language.text.clone(), claimed_extensions(config));
    }
    // Cache volumes the preflight must create + chown before any `--user` run.
    // Third field: whether the language wires a dependency-source cache. Swift is
    // the sole exception — its `launch()` passes source = None (SwiftPM checkouts
    // go to the BUILD volume via KENN_SWIFT_SCRATCH), so a swift-only workspace
    // needs the toolchain + build volumes but would leave a deps volume empty.
    let docker_langs = [
        (
            config.language.csharp.enabled,
            config.language.csharp.runtime,
            true,
        ),
        (
            config.language.rust.enabled,
            config.language.rust.runtime,
            true,
        ),
        (
            config.language.typescript.enabled,
            config.language.typescript.runtime,
            true,
        ),
        (
            config.language.python.enabled,
            config.language.python.runtime,
            true,
        ),
        (config.language.go.enabled, config.language.go.runtime, true),
        (
            config.language.swift.enabled,
            config.language.swift.runtime,
            false,
        ),
    ];
    let is_docker = |enabled: bool, runtime: kenn_config::Runtime| {
        enabled && matches!(runtime, kenn_config::Runtime::Docker)
    };
    let any_docker = docker_langs
        .iter()
        .any(|&(enabled, runtime, _)| is_docker(enabled, runtime));
    let any_deps_cache = docker_langs
        .iter()
        .any(|&(enabled, runtime, deps)| deps && is_docker(enabled, runtime));
    // Swift is provisioned HOST-side, so its version is resolved here where the
    // workspace is in hand. `swift-tools-version` is a minimum rather than an
    // exact version, so this is approximate in a way the other languages are not.
    if config.language.swift.enabled
        && matches!(config.language.swift.runtime, kenn_config::Runtime::Docker)
    {
        if let Ok(Some(pin)) =
            kenn_toolchain::pin::find_pin(kenn_toolchain::pin::Language::Swift, &ws_root)
        {
            runner.swift_toolchain = Some((format!("swift:{}", pin.version), pin.version));
        }
    }
    if any_docker {
        // The machine-wide toolchain cache, shared by every workspace. Created
        // whenever anything runs in docker, because the entrypoint provisions
        // into it regardless of whether that language has a dependency cache.
        runner
            .docker_cache_volumes
            .push(crate::docker::toolchain_volume());
        // The dependency-source volume only when some enabled docker language
        // actually wires one; otherwise (a swift-only workspace) it would be
        // created, mounted by nobody, and sit at 0 B.
        if any_deps_cache {
            runner
                .docker_cache_volumes
                .push(crate::docker::CacheVolume {
                    name: deps_volume.clone(),
                    bound_dir: deps_bound_dir.clone(),
                });
        }
        if let Some(name) = build_volume.clone() {
            runner
                .docker_cache_volumes
                .push(crate::docker::CacheVolume {
                    name,
                    bound_dir: Some(ws_root.clone()),
                });
        }
    }
    runner
}

/// The markdown walk spans the whole tree, so it must skip everything the code
/// walks skip — but this layer hardcodes NONE of it. The effective exclude set is
/// ALWAYS the union of: the workspace-internal excludes (the git dir, resolved
/// via gix rather than a hardcoded `.git`; kenn's committed store dir; and the
/// cross-language `[workspace].excludes`), every language's build/vendor
/// `excludes` (the single source of truth for `target`/`node_modules`/`.build`/…),
/// and the user's own `[language.markdown].excludes`. `[language.markdown].includes`
/// then re-includes anything wanted back (e.g. generated docs under a build dir),
/// applied at discovery. Determinism: sorted + deduped.
fn markdown_with_inherited_excludes(
    config: &Config,
    ws_root: &std::path::Path,
) -> kenn_config::MarkdownConfig {
    let mut md = config.language.markdown.clone();

    // Workspace-internal: the git dir (via gix) and kenn's committed store dir,
    // both workspace-relative and derived (never a hardcoded `.git`/`.kenn`).
    if let Some(git_glob) = git_dir_glob(ws_root) {
        md.excludes.push(git_glob);
    }
    let store = kenn_store::Layout::default_for(ws_root);
    if let Ok(rel) = store.committed_root().strip_prefix(ws_root) {
        md.excludes.push(format!("{}/**", rel.display()));
    }
    md.excludes
        .extend(config.workspace.excludes.iter().cloned());

    // Each language owns its build/vendor dirs — the single source of truth.
    for ex in [
        &config.language.rust.excludes,
        &config.language.typescript.excludes,
        &config.language.csharp.excludes,
        &config.language.python.excludes,
        &config.language.go.excludes,
        &config.language.swift.excludes,
    ] {
        md.excludes.extend(ex.iter().cloned());
    }
    md.excludes.sort();
    md.excludes.dedup();
    md
}

/// The workspace-relative `<gitdir>/**` glob for the repo containing `ws_root`,
/// or `None` outside a repo or when the git dir is not under the workspace (a
/// linked worktree's gitdir lives elsewhere and is not walked anyway).
fn git_dir_glob(ws_root: &std::path::Path) -> Option<String> {
    let git_dir = kenn_store::git::git_dir(ws_root)?;
    let wd = kenn_store::git::work_dir(ws_root).unwrap_or_else(|| ws_root.to_path_buf());
    let rel = git_dir.strip_prefix(&wd).ok()?;
    Some(format!("{}/**", rel.display()))
}

/// Every extension a language's producer claims, including satellites beyond
/// the language's own extension list: the stylesheet producer covers Sass,
/// and the TypeScript producer also claims the JS family (`tsc` indexes `.js`
/// under `allowJs`; mirrors the js→TS mapping in
/// `transform::lang::language_from_path`). Single source of truth for the
/// text fallback's claimed set AND the CLI's zero-files warning — the files
/// the fallback skips and the files the warning names must be the same set.
#[must_use]
pub fn language_claimed_extensions(language: Language) -> Vec<&'static str> {
    let mut exts: Vec<&'static str> = language.extensions().to_vec();
    match language {
        Language::Css => exts.extend(Language::Sass.extensions()),
        Language::TypeScript => exts.extend(["js", "jsx", "mjs", "cjs"]),
        _ => {}
    }
    exts
}

/// The set of source-file extensions owned by every enabled semantic / native
/// producer, so the text fallback can skip files another indexer already
/// claims (design D2 — no double-indexing). Extension-based, matching the spec:
/// e.g. an enabled `[language.rust]` claims `rs`, an enabled `[language.css]`
/// claims both css and sass extensions.
fn claimed_extensions(config: &Config) -> std::collections::BTreeSet<String> {
    let lang = &config.language;
    let enabled: [(bool, Language); 9] = [
        (lang.rust.enabled, Language::Rust),
        (lang.typescript.enabled, Language::TypeScript),
        (lang.csharp.enabled, Language::Csharp),
        (lang.python.enabled, Language::Python),
        (lang.go.enabled, Language::Go),
        (lang.swift.enabled, Language::Swift),
        (lang.markdown.enabled, Language::Markdown),
        (lang.css.enabled, Language::Css),
        (lang.html.enabled, Language::Html),
    ];
    let mut claimed = std::collections::BTreeSet::new();
    for (on, language) in enabled {
        if on {
            claimed.extend(
                language_claimed_extensions(language)
                    .iter()
                    .map(|e| (*e).to_string()),
            );
        }
    }
    claimed
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_config::Config;
    use tempfile::TempDir;

    fn workspace() -> (TempDir, Workspace) {
        let dir = TempDir::new().unwrap();
        let ws = Workspace::new(dir.path(), &[]).unwrap();
        (dir, ws)
    }

    #[test]
    fn claimed_extensions_reflect_only_enabled_producers() {
        let mut config = Config::default();
        assert!(
            claimed_extensions(&config).is_empty(),
            "all disabled → none"
        );

        config.language.rust.enabled = true;
        config.language.css.enabled = true;
        config.language.typescript.enabled = true;
        let claimed = claimed_extensions(&config);
        assert!(claimed.contains("rs"));
        // css enables both css and sass extensions.
        assert!(claimed.contains("css") && claimed.contains("scss") && claimed.contains("sass"));
        // TypeScript claims its own extensions plus the JS family (allowJs).
        assert!(claimed.contains("ts") && claimed.contains("js") && claimed.contains("jsx"));
        // A disabled producer's extensions stay unclaimed.
        assert!(!claimed.contains("go"));
    }

    #[test]
    fn configure_runner_registers_text_only_when_enabled() {
        let (_dir, ws) = workspace();
        let mut config = Config::default();
        assert!(
            configure_runner(ws, &config).text.is_none(),
            "disabled by default → no text producer"
        );

        config.language.text.enabled = true;
        config.language.text.include = vec!["**/*.yaml".into()];
        config.language.rust.enabled = true;
        let (_dir2, ws2) = workspace();
        let runner = configure_runner(ws2, &config);
        let corpus = runner.text.expect("text producer registered");
        // The claimed set (rust enabled) rode into the producer so it can skip
        // `.rs` files a real producer owns.
        assert!(corpus.claimed_exts.contains("rs"));
    }

    #[test]
    fn configure_runner_wraps_a_docker_runtime_launcher() {
        use kenn_config::Runtime;

        // A docker-runtime language's driver launches `docker`, so the phase-1
        // preflight requires it (and, for docker, a live daemon).
        let (_dir, ws) = workspace();
        let mut config = Config::default();
        config.language.rust.enabled = true;
        config.language.rust.runtime = Runtime::Docker;
        config.language.rust.image = Some("ghcr.io/kenn/ra@sha256:a".into());
        let runner = configure_runner(ws, &config);
        assert_eq!(runner.scip_drivers[0].command().to_str(), Some("docker"));
        // The per-repo dependency volume is registered (bound + to be labelled) for
        // the preflight to create+chown. Default config leaves `cache_volume` unset,
        // so it is a per-repo `kenn-deps-<hash>`, not the shared cross-repo volume.
        let deps = runner
            .docker_cache_volumes
            .iter()
            .find(|v| v.name.starts_with("kenn-deps-"))
            .expect("deps volume registered");
        assert!(deps.bound_dir.is_some(), "per-repo deps volume is bound");

        // A local-runtime language keeps its own launcher untouched, and needs
        // no docker cache volumes.
        let (_dir2, ws2) = workspace();
        let mut local = Config::default();
        local.language.rust.enabled = true;
        let local_runner = configure_runner(ws2, &local);
        assert_eq!(
            local_runner.scip_drivers[0].command().to_str(),
            Some("rust-analyzer")
        );
        assert!(local_runner.docker_cache_volumes.is_empty());
    }

    #[test]
    fn a_swift_only_docker_workspace_registers_no_deps_volume() {
        use kenn_config::Runtime;
        // Swift wires no dependency-source cache (its checkouts go to the build
        // volume), so its workspace needs the shared toolchain volume but a deps
        // volume would only ever sit empty. Mutation-checked: dropping the
        // `any_deps_cache` gate re-registers the deps volume and fails this.
        let (_dir, ws) = workspace();
        let mut config = Config::default();
        config.language.swift.enabled = true;
        config.language.swift.runtime = Runtime::Docker;
        config.language.swift.image = Some("ghcr.io/kenn/kenn-swift:v0".into());
        let runner = configure_runner(ws, &config);
        assert!(
            runner
                .docker_cache_volumes
                .iter()
                .any(|v| v.name == crate::docker::TOOLCHAIN_VOLUME),
            "docker workspace still registers the shared toolchain volume"
        );
        assert!(
            !runner
                .docker_cache_volumes
                .iter()
                .any(|v| v.name.starts_with("kenn-deps-")),
            "swift-only workspace must not register an unused deps volume: {:?}",
            runner
                .docker_cache_volumes
                .iter()
                .map(|v| &v.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn configure_runner_shared_deps_volume_is_unbound() {
        use kenn_config::Runtime;
        let (_dir, ws) = workspace();
        let mut config = Config::default();
        config.language.rust.enabled = true;
        config.language.rust.runtime = Runtime::Docker;
        config.language.rust.image = Some("img@sha256:a".into());
        config.docker.cache_volume = Some("my-shared".into());
        let runner = configure_runner(ws, &config);
        // The configured shared volume is registered verbatim and bound to nothing
        // (so `--orphans` never reaps it); no per-repo `kenn-deps-` is created.
        let shared = runner
            .docker_cache_volumes
            .iter()
            .find(|v| v.name == "my-shared")
            .expect("shared deps volume registered");
        assert!(
            shared.bound_dir.is_none(),
            "shared cross-repo volume is unbound"
        );
        assert!(!runner
            .docker_cache_volumes
            .iter()
            .any(|v| v.name.starts_with("kenn-deps-")));
    }

    #[test]
    fn deps_volume_binds_to_the_main_worktree_not_the_linked_one() {
        use kenn_config::Runtime;
        use std::process::Command;

        fn git(args: &[&str], dir: &std::path::Path) {
            let ok = Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "git {args:?} failed");
        }

        let repo = TempDir::new().unwrap();
        git(&["init", "-q", "-b", "main"], repo.path());
        git(&["config", "user.email", "t@t.invalid"], repo.path());
        git(&["config", "user.name", "t"], repo.path());
        git(&["config", "commit.gpgsign", "false"], repo.path());
        std::fs::write(repo.path().join("README"), b"x").unwrap();
        git(&["add", "."], repo.path());
        git(&["commit", "-q", "-m", "init"], repo.path());

        let wt_dir = TempDir::new().unwrap();
        let wt_path = wt_dir.path().join("feature");
        git(
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                wt_path.to_str().unwrap(),
            ],
            repo.path(),
        );

        // Index the LINKED worktree, docker runtime + persisted build cache.
        let ws = Workspace::new(&wt_path, &[]).unwrap();
        let mut config = Config::default();
        config.language.rust.enabled = true;
        config.language.rust.runtime = Runtime::Docker;
        config.language.rust.image = Some("img@sha256:a".into());
        config.docker.persist_build_cache = true;
        let runner = configure_runner(ws, &config);

        // deps binds to the repo's MAIN worktree (shared across worktrees)…
        let main = kenn_store::git::main_worktree(&wt_path).expect("main worktree resolves");
        let deps = runner
            .docker_cache_volumes
            .iter()
            .find(|v| v.name.starts_with("kenn-deps-"))
            .expect("deps volume registered");
        assert_eq!(
            deps.bound_dir.as_deref(),
            Some(main.as_path()),
            "deps binds to the main worktree, not the linked one"
        );
        // …while the build volume binds to the LINKED worktree (per-worktree).
        let build = runner
            .docker_cache_volumes
            .iter()
            .find(|v| v.name.starts_with("kenn-build-"))
            .expect("build volume registered");
        let canon_wt = std::fs::canonicalize(&wt_path).unwrap();
        assert_eq!(
            build.bound_dir.as_deref(),
            Some(canon_wt.as_path()),
            "build binds to the linked worktree"
        );
    }
}
