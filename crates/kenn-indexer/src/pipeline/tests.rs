use super::ingest::{
    is_jsonl_crash_with_no_data, record_frame_diagnostics, record_jsonl_exit_status,
    run_jsonl_with_retry, UnitCounts,
};
use super::*;

use std::path::{Path, PathBuf};

use kenn_store::DbWriter;

use crate::canonicalize::Workspace;
use crate::driver::{DriverError, IndexerDriver, ScipDriver, ScipOutcome, Unit};
use crate::report::{RunReport, RunStatus};

use tempfile::TempDir;

fn temp_writer(dir: &Path) -> DbWriter {
    let kenn = dir.join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&kenn).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(kenn_store::open_writer(
        &kenn,
        kenn_store::WriterOptions::default(),
    ))
    .expect("open_writer")
}

/// A SCIP driver that discovers one unit and reports it `Unavailable`
/// without producing a `.scip` file. Its `command` is always present
/// so preflight passes.
struct UnavailableDriver;
impl ScipDriver for UnavailableDriver {
    fn language_id(&self) -> &'static str {
        "stub"
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
        Ok(vec![Unit {
            identifier: "stub.unit".into(),
            path: PathBuf::from("/dev/null"),
        }])
    }
    fn run_unit(&self, unit: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
        let mut r = RunReport::started("stub", "0", &unit.identifier);
        r.status = RunStatus::Success;
        r.finalize();
        Ok(ScipOutcome::Unavailable { report: r })
    }
}

/// `run_jsonl_with_retry` happy path: a single ingest pass over a
/// pre-populated JSONL stream succeeds on attempt 0; the retry loop
/// does NOT respawn (Roslyn-crash detection short-circuits when
/// counts are non-zero or the status is Success).
#[test]
fn run_jsonl_with_retry_returns_on_first_attempt_for_clean_run() {
    use crate::driver::JsonlIndexer;
    use std::process::{Command, Stdio};

    struct UnusedJsonlIndexer;
    impl JsonlIndexer for UnusedJsonlIndexer {
        fn language_id(&self) -> &'static str {
            "csharp"
        }
        fn command(&self) -> PathBuf {
            PathBuf::from("true")
        }
        fn run(&self, _: &Workspace) -> Result<crate::driver::JsonlOutcome, DriverError> {
            // The happy-path test must NOT respawn — surface the
            // error rather than panicking so clippy's
            // `panic_in_result_fn` stays happy.
            Err(DriverError::Subprocess(
                "respawn must not happen on a clean run".into(),
            ))
        }
    }

    let dir = TempDir::new().unwrap();
    let source_root = dir.path().to_path_buf();
    let ws = Workspace::new(&source_root, &[]).unwrap();
    let writer = temp_writer(&source_root);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut sink = crate::sink::BatchSink::new(writer, rt.handle().clone(), 16);
    let mut registry = crate::transform::IdRegistry::new(kenn_model::Language::Csharp);

    let project_root_uri = format!("file://{}", source_root.display());
    let jsonl = format!(
        r#"{{"type":"meta","v":1,"project_root":"{project_root_uri}","tool":"kenn-dotnet","tool_version":"0.0.0","language":"csharp","ts":"2026-05-23T00:00:00.000Z"}}
{{"type":"end","stats":{{"files":0,"symbols":0,"edges":0,"errors":0}},"ts":"2026-05-23T00:00:01.000Z"}}
"#
    );
    let stream_path = source_root
        .join(".kenn")
        .join("local")
        .join("building")
        .join("stream.jsonl");
    std::fs::write(&stream_path, jsonl).unwrap();

    let child = Command::new("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut report = RunReport::started("csharp", "test", "happy");
    let counts = run_jsonl_with_retry(
        &UnusedJsonlIndexer,
        &ws,
        child,
        stream_path,
        None,
        &mut registry,
        &mut sink,
        &mut report,
    )
    .expect("run_jsonl_with_retry");
    // Success path: not crashed-with-no-data, so loop returns on
    // attempt 0 without consulting the respawn helper.
    assert!(!matches!(report.status, RunStatus::Failed));
    assert_eq!(counts.files, 0);
    // The driver seeded `"test"`; the meta frame's `tool_version` replaces it.
    // Asserted on the report itself, not on an intermediate that carries it there.
    assert_eq!(report.indexer_version, "0.0.0");
    sink.finish().unwrap();
}

/// swift-stream-indexer spec — a provisioning build-failure `ErrorFrame`
/// on the stream degrades the unit to Partial with the package attributed
/// in `failed_projects`, even though the producer exits 0. Also pins the
/// crash-retry exclusion: the error frame must not trigger a respawn
/// (the stub indexer errors if respawned).
#[test]
fn build_failure_error_frame_yields_partial_report() {
    use crate::driver::JsonlIndexer;
    use std::process::{Command, Stdio};

    struct NoRespawnIndexer;
    impl JsonlIndexer for NoRespawnIndexer {
        fn language_id(&self) -> &'static str {
            "swift"
        }
        fn command(&self) -> PathBuf {
            PathBuf::from("kenn-swift")
        }
        fn run(&self, _: &Workspace) -> Result<crate::driver::JsonlOutcome, DriverError> {
            Err(DriverError::Subprocess(
                "error frames must not trigger the crash retry".into(),
            ))
        }
    }

    let dir = TempDir::new().unwrap();
    let source_root = dir.path().to_path_buf();
    let ws = Workspace::new(&source_root, &[]).unwrap();
    let writer = temp_writer(&source_root);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut sink = crate::sink::BatchSink::new(writer, rt.handle().clone(), 16);
    let mut registry = crate::transform::IdRegistry::new(kenn_model::Language::Swift);

    let project_root_uri = format!("file://{}", source_root.display());
    let jsonl = format!(
        r#"{{"type":"meta","v":1,"project_root":"{project_root_uri}","tool":"kenn-swift","tool_version":"0.1.0","language":"swift","ts":"2026-07-06T00:00:00.000Z"}}
{{"type":"error","severity":"error","source":"build","message":"`swift build` failed; reading any existing store","path":"/ws/Pkg"}}
{{"type":"end","stats":{{"files":0,"symbols":0,"edges":0,"errors":1}},"ts":"2026-07-06T00:00:01.000Z"}}
"#
    );
    let stream_path = source_root
        .join(".kenn")
        .join("local")
        .join("building")
        .join("stream.jsonl");
    std::fs::write(&stream_path, jsonl).unwrap();

    let child = Command::new("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut report = RunReport::started("swift", "test", "build-fail");
    let counts = run_jsonl_with_retry(
        &NoRespawnIndexer,
        &ws,
        child,
        stream_path,
        None,
        &mut registry,
        &mut sink,
        &mut report,
    )
    .expect("run_jsonl_with_retry");
    assert!(matches!(report.status, RunStatus::Partial));
    assert!(
        report.failed_projects.iter().any(|p| p.contains("/ws/Pkg")),
        "failed_projects must attribute the package: {:?}",
        report.failed_projects
    );
    assert_eq!(counts.frame_errors, 1);
    sink.finish().unwrap();
}

/// Pure-helper coverage: `is_jsonl_crash_with_no_data` returns true
/// only for Partial status with zero counts; anything else is false.
#[test]
fn is_jsonl_crash_with_no_data_detects_partial_with_zero_counts() {
    let mut r = RunReport::started("csharp", "0", "u");
    r.status = RunStatus::Partial;
    assert!(is_jsonl_crash_with_no_data(
        &r,
        &UnitCounts {
            files: 0,
            symbols: 0,
            defs: 0,
            edges: 0,
            def_bodies: 0,
            frame_errors: 0,
        }
    ));
    // Non-zero counts → not a crash signature.
    assert!(!is_jsonl_crash_with_no_data(
        &r,
        &UnitCounts {
            files: 1,
            symbols: 0,
            defs: 0,
            edges: 0,
            def_bodies: 0,
            frame_errors: 0,
        }
    ));
    // Error frames observed → the producer got far enough to stream;
    // deterministic failure, not the BuildHost startup race.
    assert!(!is_jsonl_crash_with_no_data(
        &r,
        &UnitCounts {
            files: 0,
            symbols: 0,
            defs: 0,
            edges: 0,
            def_bodies: 0,
            frame_errors: 1,
        }
    ));
    // Success status → not a crash signature.
    r.status = RunStatus::Success;
    assert!(!is_jsonl_crash_with_no_data(
        &r,
        &UnitCounts {
            files: 0,
            symbols: 0,
            defs: 0,
            edges: 0,
            def_bodies: 0,
            frame_errors: 0,
        }
    ));
}

/// jsonl-indexer-driver spec — a stream with `severity: "error"` frames
/// degrades the unit to Partial and lands the attributions in
/// `failed_projects`; a `Failed` report is never upgraded.
#[test]
fn frame_errors_degrade_report_to_partial() {
    use crate::transform_jsonl::JsonlIngestStats;

    let mut report = RunReport::started("csharp", "0", "u");
    let stats = JsonlIngestStats {
        failed_errors: 2,
        failed: vec!["msbuild: App.sln: load failed".into(), "msbuild: b".into()],
        ..Default::default()
    };
    record_frame_diagnostics(&stats, &mut report);
    assert!(matches!(report.status, RunStatus::Partial));
    assert_eq!(report.failed_projects.len(), 2);
    assert!(report.failed_projects[0].contains("App.sln"));

    // Already-Failed reports keep their status.
    let mut failed = RunReport::started("csharp", "0", "u");
    failed.status = RunStatus::Failed;
    record_frame_diagnostics(&stats, &mut failed);
    assert!(matches!(failed.status, RunStatus::Failed));

    // No severity-error frames → untouched.
    let mut clean = RunReport::started("csharp", "0", "u");
    record_frame_diagnostics(&JsonlIngestStats::default(), &mut clean);
    assert!(matches!(clean.status, RunStatus::Success));
    assert!(clean.failed_projects.is_empty());
}

/// Warning-severity frames land in `report.warnings` (status-neutral) —
/// they carry producer degradation notices that `kenn status` must show,
/// so they cannot die in a counter.
#[test]
fn warning_frames_are_surfaced_status_neutral() {
    use crate::transform_jsonl::JsonlIngestStats;

    let mut report = RunReport::started("kenn-swift", "0", "u");
    let stats = JsonlIngestStats {
        warning_total: 40,
        warned: (0..32).map(|i| format!("store: pkg{i}: stale")).collect(),
        ..Default::default()
    };
    record_frame_diagnostics(&stats, &mut report);
    assert!(
        matches!(report.status, RunStatus::Success),
        "warnings never degrade"
    );
    assert_eq!(report.warnings.len(), 32);
    assert_eq!(report.warnings_overflow, 8);
    assert!(report.failed_projects.is_empty());
}

/// Attributions past the cap surface as structured overflow, never as a
/// synthetic list entry — counting consumers must not mistake a summary
/// marker for a real failure.
#[test]
fn frame_error_overflow_is_structured() {
    use crate::transform_jsonl::JsonlIngestStats;

    let mut report = RunReport::started("csharp", "0", "u");
    let stats = JsonlIngestStats {
        failed_errors: 40,
        failed: (0..32).map(|i| format!("msbuild: sln{i}")).collect(),
        ..Default::default()
    };
    record_frame_diagnostics(&stats, &mut report);
    assert_eq!(report.failed_projects.len(), 32);
    assert_eq!(report.failed_overflow, 8);
    assert!(
        !report.failed_projects.iter().any(|p| p.contains("more")),
        "no synthetic marker in the data"
    );
    // The marker exists only in rendered output.
    let rendered =
        crate::report::render_with_overflow(&report.failed_projects, report.failed_overflow);
    assert_eq!(rendered.len(), 33);
    assert_eq!(rendered[32], "+8 more");
}

/// A non-zero exit names the report's producer (`indexer_name`) — stable
/// even when the configured command is a runner form like
/// `["dotnet", "kenn-dotnet.dll"]`.
#[cfg(unix)]
#[test]
fn exit_status_message_names_the_producer() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    let mut report = RunReport::started("kenn-ts", "0", "u");
    record_jsonl_exit_status(Ok(ExitStatus::from_raw(1 << 8)), "boom", &mut report);
    assert!(matches!(report.status, RunStatus::Partial));
    assert!(
        report.failed_projects[0].starts_with("kenn-ts exit 1"),
        "got: {}",
        report.failed_projects[0]
    );
}

/// The failure entry must OPEN with the cause, not bury it.
///
/// This path used to append the raw 8KB stderr tail alone. An agent reading
/// `failed_projects` over a tool call gets the first line; if that line is
/// "kenn-swift exit 1" and the actual reason is 40 lines down inside build
/// noise, the relay has technically preserved the information and practically
/// lost it.
///
/// The tail must still be there — a person debugging a broken toolchain wants
/// the surrounding output — so this asserts BOTH, not one at the expense of the
/// other.
#[cfg(unix)]
#[test]
fn exit_status_message_leads_with_the_cause_and_keeps_the_tail() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    // The real shape: progress output, the cause mid-stream, then a backtrace.
    // `lines().last()` would pick the frame, which is why extraction prefers
    // the first `error` line.
    let tail = "Building for debugging...\n\
                error: no such module 'IndexStore'\n\
                note: check your toolchain\n\
                stack backtrace:\n   6: __pthread_cond_wait";
    let mut report = RunReport::started("kenn-swift", "0", "u");
    record_jsonl_exit_status(Ok(ExitStatus::from_raw(1 << 8)), tail, &mut report);

    let entry = &report.failed_projects[0];
    let first = entry.lines().next().unwrap_or_default();
    assert!(
        first.contains("no such module 'IndexStore'"),
        "first line must carry the cause, got: {first}"
    );
    assert!(
        !first.contains("__pthread_cond_wait"),
        "must not lead with a backtrace frame: {first}"
    );
    assert!(
        entry.contains("Building for debugging"),
        "the tail must be retained: {entry}"
    );
}

#[test]
fn pipeline_finalizes_even_with_no_units() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let runner = IndexerDriver::new(ws);
    let writer = temp_writer(dir.path());
    let (reports, _writer) = run_pipeline(&runner, writer, 100).unwrap();
    assert!(reports.is_empty());
}

#[test]
fn pipeline_collects_unavailable_reports() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let runner = IndexerDriver::new(ws).with_scip_driver(UnavailableDriver);
    let writer = temp_writer(dir.path());
    let (reports, _writer) = run_pipeline(&runner, writer, 100).unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].indexer_name, "stub");
}

/// Write a minimal `.scip` index: one `Metadata` frame carrying `project_root`
/// and one `Document` at `rel_path`. Enough to drive the ingest transform's
/// canonicalization down the out-of-root path when `project_root` disagrees
/// with the workspace root.
fn write_scip_index(path: &Path, project_root_uri: &str, rel_path: &str, language: &str) {
    use protobuf::Message;
    use scip::types::{Document, Index, Metadata};
    let mut metadata = Metadata::new();
    metadata.project_root = project_root_uri.to_string();
    let mut doc = Document::new();
    doc.relative_path = rel_path.to_string();
    doc.language = language.to_string();
    let mut index = Index::new();
    index.metadata = protobuf::MessageField::some(metadata);
    index.documents = vec![doc];
    std::fs::write(path, index.write_to_bytes().expect("encode scip")).expect("write scip");
}

/// A SCIP driver that returns a pre-written `.scip` file (see `write_scip_index`)
/// as its unit output — used to feed the ingest path a controlled index without
/// a real indexer.
struct FixtureScipDriver {
    scip: PathBuf,
}
impl ScipDriver for FixtureScipDriver {
    fn language_id(&self) -> &'static str {
        "rust"
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
        Ok(vec![Unit {
            identifier: "Cargo.toml".into(),
            path: PathBuf::from("/dev/null"),
        }])
    }
    fn run_unit(&self, unit: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
        let report = RunReport::started_for(
            kenn_model::Language::Rust,
            "rust-analyzer",
            "?",
            &unit.identifier,
        );
        Ok(ScipOutcome::Scip {
            path: self.scip.clone(),
            report,
        })
    }
}

/// Foundation (out-of-root reporting): a SCIP whose `project_root` disagrees with
/// the workspace root drops every document at canonicalization. That drop must be
/// counted and named in the report, not silently swallowed — otherwise the run
/// publishes an empty index and exits 0.
#[test]
fn scip_documents_outside_the_root_are_counted_not_silently_skipped() {
    let dir = TempDir::new().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    // project_root `/work` is unrelated to the temp workspace root: the document
    // resolves to /work/src/main.rs, which strip_prefix'es outside the root.
    let scip = dir.path().join("bad.scip");
    write_scip_index(&scip, "file:///work", "src/main.rs", "rust");
    let runner = IndexerDriver::new(ws).with_scip_driver(FixtureScipDriver { scip });
    let writer = temp_writer(dir.path());
    let (reports, _writer) = run_pipeline(&runner, writer, 100).unwrap();

    let r = reports
        .iter()
        .find(|r| r.indexer_name == "rust-analyzer")
        .expect("a rust-analyzer unit report");
    assert!(
        r.out_of_root_seen >= 1,
        "the out-of-root document must be counted, got report {r:?}"
    );
    assert_eq!(r.files_seen, 0, "no in-root document was indexed");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("outside the workspace root")),
        "a warning names the mismatch, got {:?}",
        r.warnings
    );
    assert!(
        matches!(r.status, RunStatus::Partial),
        "status degrades to Partial, got {:?}",
        r.status
    );
    // The run-level tripwire both orchestrators consult fires on these real
    // reports (all dropped, none kept) — the decision, not just the counter.
    assert_eq!(
        crate::report::all_documents_outside_root(&reports),
        Some(1),
        "the shared tripwire fires on a fully out-of-root run"
    );
}

#[test]
fn progress_callback_is_called_at_milestones() {
    use std::sync::Mutex;
    let dir = TempDir::new().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let runner = IndexerDriver::new(ws);
    let writer = temp_writer(dir.path());
    let events: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let progress = |e: ProgressEvent| {
        let tag = match e {
            ProgressEvent::Started => "started",
            ProgressEvent::UnitIngested { .. } => "unit",
            ProgressEvent::StubsFlushed { .. } => "stubs",
            ProgressEvent::AggregateComputed { .. } => "aggregate",
            ProgressEvent::EndRunComplete { .. } => "end_run",
            ProgressEvent::Completed { .. } => "completed",
        };
        events.lock().unwrap().push(tag.into());
    };
    let _ = run_pipeline_with_progress(&runner, writer, 100, progress, no_op_hook(), None).unwrap();
    let got = events.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![
            "started".to_string(),
            "stubs".into(),
            "aggregate".into(),
            "end_run".into(),
            "completed".into(),
        ],
    );
}

#[test]
fn preflight_fails_when_a_required_cli_is_missing() {
    struct MissingCliDriver;
    impl ScipDriver for MissingCliDriver {
        fn language_id(&self) -> &'static str {
            "rust"
        }
        fn command(&self) -> PathBuf {
            PathBuf::from("/nonexistent/kenn-preflight-xyz")
        }
        fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
            Ok(Vec::new())
        }
        fn run_unit(&self, _: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
            Err(DriverError::Subprocess(
                "preflight fails before run_unit".into(),
            ))
        }
    }
    let dir = TempDir::new().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let runner = IndexerDriver::new(ws).with_scip_driver(MissingCliDriver);
    let writer = temp_writer(dir.path());
    let result = run_pipeline(&runner, writer, 100);
    assert!(matches!(result, Err(PipelineError::MissingCli(_))));
}

/// 6.1 — markdown ingest runs as a sibling pipeline unit: a markdown-only
/// run completes, emits a `markdown` Success report, and the md↔md graph is
/// resolved without any code drivers present.
#[test]
fn markdown_unit_runs_in_pipeline() {
    use crate::driver::IndexerDriver;
    use kenn_config::{MarkdownConfig, MarkdownRoot};

    let dir = TempDir::new().unwrap();
    let src = dir.path();
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.md"), "# A\nsee [[b]]\n").unwrap();
    std::fs::write(src.join("docs/b.md"), "# B\n").unwrap();

    let ws = Workspace::new(src, &[]).unwrap();
    let cfg = MarkdownConfig {
        enabled: true,
        roots: vec![MarkdownRoot {
            glob: "docs".into(),
            label: None,
        }],
        excludes: vec![],
        includes: vec![],
    };
    let runner = IndexerDriver::new(ws).with_markdown(cfg);
    let writer = temp_writer(src);

    let (reports, _writer) = run_pipeline(&runner, writer, 16).expect("pipeline runs");
    let md = reports
        .iter()
        .find(|r| r.indexer_name == "markdown")
        .expect("a markdown report");
    assert_eq!(md.status, RunStatus::Success);
}

/// 6.4 — end-to-end HTML+CSS+JS index through the real 4-phase pipeline: the
/// HTML producer emits the document/id/inline nodes in the parallel phase, and
/// the post-code/CSS barrier resolves links, imports, correspondence, and class
/// usage against the building store. Asserts every edge class resolves.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end fixture: seed JS, run the pipeline, then assert each edge class in sequence; splitting would scatter the shared writer/reader/runtime"
)]
fn html_css_js_indexes_end_to_end() {
    use kenn_config::{CssConfig, HtmlConfig};
    use kenn_model::{
        compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language,
        SymbolRecord,
    };
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let src = dir.path();
    std::fs::write(
        src.join("index.html"),
        "<!doctype html>\n\
         <link rel=\"stylesheet\" href=\"app.css\">\n\
         <script src=\"app.js\"></script>\n\
         <a href=\"about.html\">about</a>\n\
         <style>.inline-c { color: green }</style>\n\
         <div id=\"hero\" class=\"btn\"><img src=\"logo.png\"></div>\n",
    )
    .unwrap();
    std::fs::write(src.join("about.html"), "<html><body>about</body></html>\n").unwrap();
    std::fs::write(
        src.join("app.css"),
        ".btn { color: red }\n#hero { top: 0 }\n",
    )
    .unwrap();
    std::fs::write(src.join("app.js"), "export const x = 1;\n").unwrap();
    std::fs::write(src.join("logo.png"), "PNG").unwrap();

    let ws = Workspace::new(src, &[]).unwrap();
    let writer = temp_writer(src);

    // Seed a JS file node so `<script src="app.js">` resolves to a real import
    // target (no JS indexer runs in this harness).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let js_file = compose_short_id(Language::TypeScript, 1);
    let js_mod = compose_short_id(Language::TypeScript, 2);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: js_file,
        path: "app.js".into(),
        language: Language::TypeScript,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: js_mod,
        pub_id: "ts:app.js".into(),
        language: Language::TypeScript,
        pkg_id: 0,
        kind: Kind::Module,
        name: "app.js".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: js_mod,
        file_id: js_file,
        start_line: 1,
        start_col: 0,
        end_line: 1,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    b.edges.push(EdgeRecord {
        src_id: js_mod,
        target_id: js_file,
        properties: EdgeProperties::Contains,
    });
    rt.block_on(writer.write_batch(&b)).expect("seed js file");

    let runner = IndexerDriver::new(ws)
        .with_css(CssConfig {
            enabled: true,
            roots: vec![".".into()],
            ..Default::default()
        })
        .with_html(HtmlConfig {
            enabled: true,
            roots: vec![".".into()],
            ..Default::default()
        });

    let (reports, out_writer) = run_pipeline(&runner, writer, 16).expect("pipeline runs");
    assert_eq!(
        reports
            .iter()
            .find(|r| r.indexer_name == "html")
            .map(|r| &r.status),
        Some(&RunStatus::Success),
    );
    assert_eq!(
        reports
            .iter()
            .find(|r| r.indexer_name == "html-resolve")
            .map(|r| &r.status),
        Some(&RunStatus::Success),
    );

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&out_writer))
        .expect("reader");

    // document + html_id nodes
    assert!(rt
        .block_on(Reader::fetch_symbol(&reader, "html", "html:index.html"))
        .unwrap()
        .is_some());
    let hero = rt
        .block_on(Reader::fetch_symbol(
            &reader,
            "html",
            "html:index.html#id:hero",
        ))
        .unwrap()
        .expect("hero html_id");
    // inline-style css node owned by the HTML file (css: prefix, html relpath),
    // registered in the shared class registry
    assert!(rt
        .block_on(Reader::fetch_symbol(
            &reader,
            "css",
            "css:index.html#class:inline-c",
        ))
        .unwrap()
        .is_some());

    // link edge → about.html (links_to_file)
    let about = rt
        .block_on(Reader::fetch_symbol(&reader, "html", "html:about.html"))
        .unwrap()
        .expect("about doc");
    let (_r, links) = rt
        .block_on(Reader::list_inbound(
            &reader,
            about.id,
            "links_to_file",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .unwrap();
    assert_eq!(links, 1, "index.html → about.html");

    // import edges → app.css (CSS) and app.js (JS)
    let appcss = rt
        .block_on(Reader::fetch_symbol(&reader, "css", "css:app.css"))
        .unwrap()
        .expect("app.css module");
    let (_r, css_imp) = rt
        .block_on(Reader::list_inbound(
            &reader,
            appcss.id,
            "imports",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .unwrap();
    assert_eq!(css_imp, 1, "index.html imports app.css");
    let (_r, js_imp) = rt
        .block_on(Reader::list_inbound(
            &reader,
            js_file,
            "imports",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .unwrap();
    assert_eq!(js_imp, 1, "index.html imports app.js");

    // correspondence hero ↔ #hero
    let (_r, corr) = rt
        .block_on(Reader::list_outbound(
            &reader,
            hero.id,
            "corresponds_to",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .unwrap();
    assert_eq!(corr, 1, "hero html_id corresponds to #hero css_id");

    // class usage btn, attributed to the enclosing `hero` html_id
    let btn = rt
        .block_on(reader.symbols_by_short_name("btn"))
        .unwrap()
        .into_iter()
        .find(|h| h.qualified == "css:app.css#class:btn")
        .expect("btn class node")
        .id;
    let (use_rows, uses) = rt
        .block_on(Reader::list_inbound(
            &reader,
            btn,
            "uses_css_class",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .unwrap();
    assert_eq!(uses, 1, "btn used once, from the hero html_id");
    assert_eq!(use_rows[0].pub_id, "html:index.html#id:hero");
}

#[test]
fn the_code_table_report_distinguishes_scanned_from_found() {
    // Three numbers, and each must land on its own field. The failure this
    // guards is not a crash: mapping one count to the wrong field, or dropping
    // one, leaves a report that still parses and still looks complete — and a
    // reader then cannot tell a table nothing touches from one whose access
    // this pass could not see.
    let counts = crate::code_sql::resolve::CodeSqlCounts {
        bodies_scanned: 9309,
        bodies_with_literals: 4103,
        refs_emitted: 252,
        tables_minted: 40,
        refs_dropped: 0,
    };
    let mut r = RunReport::started("code-tables", "0", "<corpus>");
    super::api::record_code_table_counts(&mut r, &counts);

    assert_eq!(r.def_bodies_seen, 9309, "bodies scanned");
    assert_eq!(r.bodies_with_literals, 4103, "bodies carrying literals");
    assert_eq!(r.edges_seen, 252, "references emitted");
    assert_eq!(r.symbols_seen, 40, "tables minted");
    assert_eq!(
        r.files_seen, 0,
        "files_seen rolls up into the per-language file total `kenn index` \
         prints — this step re-reads files another unit already counted"
    );
}

/// The XML↔SQL bridge, end to end through the real pipeline.
///
/// Both halves of a workspace's schema in one fixture: a `.sql` migration that
/// declares a table, and XML that reaches the same table by attribute and names
/// a second one that no statement declares.
#[test]
fn the_xml_sql_bridge_joins_both_surfaces_to_one_table_graph() {
    let dir = TempDir::new().unwrap();
    let src = dir.path();
    std::fs::write(src.join("schema.sql"), "CREATE TABLE users (id INT);\n").unwrap();
    std::fs::write(
        src.join("changelog.xml"),
        // One element reaches `users` by attribute; one reaches `orders`, which
        // nothing declares, so it must be minted; one carries SQL as text.
        "<changelog>\
           <createTable tableName=\"users\"/>\
           <createTable tableName=\"orders\"/>\
           <sql>SELECT id FROM users WHERE id &gt; 1</sql>\
         </changelog>\n",
    )
    .unwrap();

    let ws = Workspace::new(src, &[]).unwrap();
    let writer = temp_writer(src);
    let runner = IndexerDriver::new(ws)
        .with_sql(kenn_config::SqlConfig {
            enabled: true,
            ..Default::default()
        })
        .with_xml(kenn_config::XmlConfig {
            enabled: true,
            ..Default::default()
        })
        .with_xml_sql(kenn_config::XmlSqlConfig {
            rules: vec![kenn_config::TableRule {
                attribute: "tableName".into(),
                element: Some("createTable".into()),
                role: Some(kenn_config::TableRole::Declares),
            }],
            ..Default::default()
        });

    let (reports, out_writer) = run_pipeline(&runner, writer, 16).expect("pipeline runs");

    // 2.3: the step reports separately from every producer, so a failure here
    // would degrade only this report.
    let bridge = reports
        .iter()
        .find(|r| r.indexer_name == "xml-tables")
        .expect("the bridge files its own report");
    assert_eq!(bridge.status, RunStatus::Success);
    assert!(bridge.edges_seen > 0, "it emitted references");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&out_writer))
        .expect("reader");
    let symbols = rt
        .block_on(kenn_store::api::Reader::scan_symbols(&reader))
        .expect("scan");

    let tables: Vec<&str> = symbols
        .iter()
        .filter(|s| s.kind == kenn_model::Kind::SqlTable.db_name())
        .map(|s| s.pub_id.as_str())
        .collect();
    assert!(
        tables.contains(&"sql:users"),
        "the declared table: {tables:?}"
    );
    assert!(
        tables.contains(&"sql:orders"),
        "5.3: a table named only by an attribute is minted: {tables:?}"
    );

    // 5.1 + 5.3: the attribute reference and the `.sql` declaration reach ONE
    // node, and the XML edges come from elements rather than the document.
    let users = symbols
        .iter()
        .find(|s| s.pub_id == "sql:users")
        .expect("users node");
    let (rows, _) = rt
        .block_on(kenn_store::api::Reader::list_inbound(
            &reader,
            users.id,
            "defines_table",
            50,
            None,
            &kenn_store::RowNarrow::visibility(true, true),
        ))
        .expect("inbound");
    let sources: Vec<&str> = rows.iter().map(|r| r.pub_id.as_str()).collect();
    assert!(
        sources.iter().any(|p| p.starts_with("sql:")),
        "the statement declared it: {sources:?}"
    );
    assert!(
        sources.iter().any(|p| p.starts_with("xml:")),
        "and the element did too, on the same node: {sources:?}"
    );
    assert!(
        sources.iter().all(|p| !p.ends_with("changelog.xml")),
        "2.1/5.1: from the element, never the document: {sources:?}"
    );
}

/// 2.2: a workspace with XML but no tables anywhere still indexes cleanly, and
/// the step reports nothing rather than a degraded run.
#[test]
fn a_workspace_whose_xml_names_no_table_skips_the_bridge_cleanly() {
    let dir = TempDir::new().unwrap();
    let src = dir.path();
    std::fs::write(
        src.join("config.xml"),
        "<config><timeout>30</timeout><name>svc</name></config>\n",
    )
    .unwrap();

    let ws = Workspace::new(src, &[]).unwrap();
    let writer = temp_writer(src);
    let runner = IndexerDriver::new(ws)
        .with_xml(kenn_config::XmlConfig {
            enabled: true,
            ..Default::default()
        })
        .with_xml_sql(kenn_config::XmlSqlConfig::default());

    let (reports, _writer) = run_pipeline(&runner, writer, 16).expect("pipeline runs");
    let bridge = reports.iter().find(|r| r.indexer_name == "xml-tables");
    // It ran (there were elements) and found nothing — success with no edges,
    // not a failure and not a claim of work it did not do.
    if let Some(r) = bridge {
        assert_eq!(r.status, RunStatus::Success);
        assert_eq!(r.edges_seen, 0);
        assert_eq!(r.symbols_seen, 0, "nothing minted from non-SQL content");
    }
    assert!(
        reports.iter().all(|r| r.status != RunStatus::Failed),
        "no unit degraded: {:?}",
        reports.iter().map(|r| &r.indexer_name).collect::<Vec<_>>()
    );
}
