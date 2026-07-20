//! Verification: concurrent language ingesters append directly to the
//! Lance datasets (retire-redb D9). The ingester-to-DB-writer channel
//! and the single DB-writer thread are gone — each ingester owns a
//! `BatchSink` and appends through it; Lance's optimistic-concurrency
//! commit guard resolves the concurrent appends.

use kenn_indexer::driver::IndexerDriver;
use kenn_indexer::driver::{DriverError, ScipDriver, ScipOutcome, Unit};
use kenn_indexer::report::RunReport;
use kenn_indexer::sink::BatchSink;
use kenn_indexer::transform::IdRegistry;
use kenn_indexer::transform_jsonl::{ingest_jsonl_from_growing_file, ingest_jsonl_into_sink};
use kenn_indexer::{run_pipeline_with_progress, Workspace};
use kenn_model::{compose_short_id, partition_of, Kind, Language, SymbolRecord};
use kenn_store::{open_writer, WriterOptions};
use std::io::Cursor;
use std::path::PathBuf;
use tempfile::TempDir;

fn symbol(language: Language, counter: u32) -> SymbolRecord {
    SymbolRecord {
        id: compose_short_id(language, counter),
        pub_id: format!("{}:s{counter}", language.prefix()),
        language,
        pkg_id: 0,
        kind: Kind::Function,
        name: format!("s{counter}"),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

/// Several language ingesters run concurrently and append directly,
/// each through its own `BatchSink`. After all finish, the code graph
/// holds the union, with every `short_id` in its own language partition
/// — and no append is lost to a commit conflict.
#[test]
fn concurrent_ingesters_produce_a_partitioned_union() {
    const PER: u32 = 60;
    let langs = [
        Language::Rust,
        Language::Go,
        Language::TypeScript,
        Language::Python,
    ];

    let dir = TempDir::new().unwrap();
    let building = dir.path().join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&building).expect("create building dir");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let writer = rt
        .block_on(open_writer(&building, WriterOptions::default()))
        .expect("open_writer");
    let handle = rt.handle().clone();

    // One ingester per language, each on its own OS thread, each with
    // its own writer clone — appending concurrently to shared datasets.
    std::thread::scope(|scope| {
        for &language in &langs {
            let mut sink = BatchSink::new(writer.clone(), handle.clone(), 16);
            scope.spawn(move || {
                for counter in 1..=PER {
                    sink.push_symbol(symbol(language, counter))
                        .expect("push_symbol");
                }
                sink.finish().expect("ingester finish");
            });
        }
    });

    // The code graph holds the union, partitioned disjointly by language.
    let symbols = rt
        .block_on(writer.scan_symbols_for_aggregation())
        .expect("scan symbols");
    assert_eq!(symbols.len(), langs.len() * PER as usize);
    for &language in &langs {
        let in_partition = symbols
            .iter()
            .filter(|s| partition_of(s.id) == language.partition())
            .count();
        assert_eq!(
            in_partition, PER as usize,
            "{language:?} partition holds exactly its own ids"
        );
    }
}

/// `ingest_jsonl_into_sink` drives `StreamState` + `handle_frame`
/// end-to-end. This fixture sends one of every `Frame` variant
/// through the pipeline:
/// Meta → File → Package → Stub → Symbol (×2 — one fresh, one
/// same-wire-id repeat so `on_symbol`'s dedup branch fires) → Edge →
/// Error → End.
/// Verifies the stats match the frames consumed.
#[test]
fn ingest_jsonl_into_sink_drives_every_frame_kind() {
    let dir = TempDir::new().unwrap();
    let source_root = dir.path().join("ws");
    std::fs::create_dir_all(&source_root).unwrap();
    let building = source_root.join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&building).unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let writer = rt
        .block_on(open_writer(&building, WriterOptions::default()))
        .expect("open_writer");
    let handle = rt.handle().clone();
    let mut sink = BatchSink::new(writer, handle, 16);

    let workspace = Workspace::new(&source_root, &[]).expect("workspace");
    let mut registry = IdRegistry::new(Language::Csharp);

    // A minimal but exhaustive JSONL stream (one frame per Frame variant).
    let project_root_uri = format!("file://{}", source_root.display());
    let jsonl = format!(
        r#"{{"type":"meta","v":1,"project_root":"{project_root_uri}","tool":"kenn-dotnet","tool_version":"0.0.0","language":"csharp","ts":"2026-05-23T00:00:00.000Z"}}
{{"type":"file","id":1,"path":"src/Main.cs","content_hash":"abc"}}
{{"type":"package","id":1,"name":"TestPkg","version":"0.0.0","manager":"nuget"}}
{{"type":"stub","id":2,"kind":"class","name":"StubClass","key":"TestPkg.StubClass","pkg":1}}
{{"type":"symbol","id":3,"pkg":1,"key":"TestPkg.Main","kind":"class","name":"Main","file":1,"range":[0,0,10,0]}}
{{"type":"symbol","id":3,"pkg":1,"key":"TestPkg.Main","kind":"class","name":"Main","file":1,"range":[0,0,10,0]}}
{{"type":"edge","edge_kind":"calls","source":3,"target":2}}
{{"type":"error","severity":"warn","source":"test","message":"synthetic"}}
{{"type":"end","stats":{{"files":1,"symbols":1,"edges":1,"errors":1}},"ts":"2026-05-23T00:00:01.000Z"}}
"#
    );
    let mut reader = Cursor::new(jsonl.into_bytes());

    let stats = ingest_jsonl_into_sink(&mut reader, &workspace, &mut registry, &mut sink)
        .expect("ingest_jsonl_into_sink");

    // Every frame kind was handled — the counters reflect what
    // handle_frame routed through StreamState's on_* methods:
    assert!(stats.files >= 1, "at least one file frame consumed");
    assert!(stats.packages >= 1, "at least one package frame consumed");
    assert!(stats.symbols >= 1, "at least one symbol frame consumed");
    assert!(stats.edges >= 1, "at least one edge frame consumed");
    assert_eq!(stats.errors, 1, "the error frame is recorded as 1 error");
    // The meta frame's `tool_version` reaches the caller, which uses it to
    // replace the unit report's placeholder `indexer_version` of "?".
    assert_eq!(
        stats.tool_version.as_deref(),
        Some("0.0.0"),
        "the producer's version is captured from the meta frame"
    );

    sink.finish().expect("sink finish");
}

/// `ingest_jsonl_from_growing_file` end-to-end: a real on-disk JSONL
/// file populated up-front, plus a real exited `Child` (we spawn
/// `true`). The function reads the file in a single pass, drives the
/// EOF-poll branch once, then exits via the `End` frame.
#[test]
fn ingest_jsonl_from_growing_file_drives_full_stream() {
    use std::process::{Command, Stdio};

    let dir = TempDir::new().unwrap();
    let source_root = dir.path().join("ws");
    std::fs::create_dir_all(&source_root).unwrap();
    let building = source_root.join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&building).unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let writer = rt
        .block_on(open_writer(&building, WriterOptions::default()))
        .unwrap();
    let mut sink = BatchSink::new(writer, rt.handle().clone(), 16);

    let workspace = Workspace::new(&source_root, &[]).unwrap();
    let mut registry = IdRegistry::new(Language::Csharp);

    let project_root_uri = format!("file://{}", source_root.display());
    let jsonl = format!(
        r#"{{"type":"meta","v":1,"project_root":"{project_root_uri}","tool":"kenn-dotnet","tool_version":"0.0.0","language":"csharp","ts":"2026-05-23T00:00:00.000Z"}}
{{"type":"file","id":1,"path":"src/A.cs","content_hash":"h1"}}
{{"type":"package","id":1,"name":"Pkg","version":"0.0.0","manager":"nuget"}}
{{"type":"symbol","id":2,"pkg":1,"key":"Pkg.A","kind":"class","name":"A","file":1,"range":[0,0,1,0]}}
{{"type":"end","stats":{{"files":1,"symbols":1,"edges":0,"errors":0}},"ts":"2026-05-23T00:00:01.000Z"}}
"#
    );
    let stream_path = building.join("stream.jsonl");
    std::fs::write(&stream_path, jsonl).unwrap();

    // A real Child that exits immediately. `try_wait` on it during
    // `next_jsonl_line` returns Some after a single sleep cycle,
    // exercising the post-exit drain branch.
    let mut child = Command::new("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let (stats, end_frame_seen) = ingest_jsonl_from_growing_file(
        &stream_path,
        &mut child,
        &workspace,
        &mut registry,
        &mut sink,
    )
    .expect("ingest_jsonl_from_growing_file");

    assert!(end_frame_seen, "End frame seen");
    assert!(stats.files >= 1);
    assert!(stats.symbols >= 1);

    sink.finish().expect("sink finish");
    child.wait().expect("child wait");
}

/// A producer that flushes MID-RECORD must not corrupt the ingest.
///
/// Regression: `next_jsonl_line` accepted whatever `read_line` returned, but
/// `read_line` also stops at EOF — so a half-written record came back as if it
/// were a complete line and went straight to the JSON parser ("EOF while parsing
/// a string"). Real producers hit this constantly: a pipe flushes at a 64KiB
/// boundary, which lands mid-record on any large workspace, and the whole ingest
/// failed with 0 files indexed.
///
/// The sibling test above writes the file COMPLETE before reading, so it can
/// never catch this. Here the tail of a record — and the `end` frame — arrive
/// only after the reader has already seen the fragment.
#[test]
fn ingest_jsonl_tolerates_a_producer_flushing_mid_record() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let dir = TempDir::new().unwrap();
    let source_root = dir.path().join("ws");
    std::fs::create_dir_all(&source_root).unwrap();
    let building = source_root.join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&building).unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let writer = rt
        .block_on(open_writer(&building, WriterOptions::default()))
        .unwrap();
    let mut sink = BatchSink::new(writer, rt.handle().clone(), 16);

    let workspace = Workspace::new(&source_root, &[]).unwrap();
    let mut registry = IdRegistry::new(Language::Csharp);
    let project_root_uri = format!("file://{}", source_root.display());

    // Everything up to a SPLIT POINT inside the symbol record's `key` string.
    let head = format!(
        r#"{{"type":"meta","v":1,"project_root":"{project_root_uri}","tool":"kenn-dotnet","tool_version":"0.0.0","language":"csharp","ts":"2026-05-23T00:00:00.000Z"}}
{{"type":"file","id":1,"path":"src/A.cs","content_hash":"h1"}}
{{"type":"symbol","id":2,"key":"Pkg."#
    );
    let tail = r#"A","kind":"class","name":"A","file":1,"range":[0,0,1,0]}
{"type":"end","stats":{"files":1,"symbols":1,"edges":0,"errors":0},"ts":"2026-05-23T00:00:01.000Z"}
"#;

    let stream_path = building.join("stream.jsonl");
    std::fs::write(&stream_path, &head).unwrap();

    // Keep the "producer" alive long enough that the reader observes the
    // fragment and must poll for the rest.
    let mut child = Command::new("sleep")
        .arg("0.5")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let append_path = stream_path.clone();
    let appender = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&append_path)
            .unwrap();
        f.write_all(tail.as_bytes()).unwrap();
        f.flush().unwrap();
    });

    let (stats, end_frame_seen) = ingest_jsonl_from_growing_file(
        &stream_path,
        &mut child,
        &workspace,
        &mut registry,
        &mut sink,
    )
    .expect("a mid-record flush must not fail the ingest");

    appender.join().unwrap();
    assert!(
        end_frame_seen,
        "the end frame arrives after the split and must still be seen"
    );
    assert_eq!(stats.files, 1, "the file frame survives");
    assert_eq!(
        stats.symbols, 1,
        "the record split across two flushes is parsed once, intact"
    );

    sink.finish().expect("sink finish");
    child.wait().expect("child wait");
}

/// Stub SCIP driver that returns a path to a real on-disk .scip file
/// pre-populated by the test. Drives `ingest_scip_driver` end-to-end
/// through `run_pipeline_with_progress`.
struct SyntheticScipDriver {
    language: &'static str,
    scip_path: PathBuf,
}

impl ScipDriver for SyntheticScipDriver {
    fn language_id(&self) -> &str {
        self.language
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
        Ok(vec![Unit {
            identifier: format!("{}.unit", self.language),
            path: self.scip_path.clone(),
        }])
    }
    fn run_unit(&self, _: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
        let mut r = RunReport::started(self.language, "stub", "synth");
        r.finalize();
        Ok(ScipOutcome::Scip {
            path: self.scip_path.clone(),
            report: r,
        })
    }
}

fn write_synthetic_scip(path: &std::path::Path, language: &str, file_rel: &str, symbol: &str) {
    use protobuf::Message;
    use scip::types::{Document, Index, Metadata, Occurrence, SymbolInformation, ToolInfo};
    let mut idx = Index::new();
    let mut md = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "test".into();
    tool.version = "0.0.0".into();
    md.tool_info = protobuf::MessageField::some(tool);
    let canonical = path.parent().and_then(|p| p.parent()).map_or_else(
        || "file:///tmp".into(),
        |p| format!("file://{}", p.display()),
    );
    md.project_root = canonical;
    idx.metadata = protobuf::MessageField::some(md);
    let mut doc = Document::new();
    doc.language = language.into();
    doc.relative_path = file_rel.into();
    let mut sym = SymbolInformation::new();
    sym.symbol = symbol.into();
    doc.symbols.push(sym);
    let mut occ = Occurrence::new();
    occ.range = vec![0_i32, 0, 0, 0];
    occ.symbol = symbol.into();
    doc.occurrences.push(occ);
    idx.documents.push(doc);
    let bytes = idx.write_to_bytes().expect("write_to_bytes");
    std::fs::write(path, bytes).expect("write scip file");
}

/// Build a SCIP file with one locally-defined workspace symbol that
/// references an external (no-definition) symbol. After the
/// `external-edges-in-scip-graph` change, the external symbol SHALL
/// appear in the symbols table with `external = true` and a `calls`
/// edge SHALL link the workspace symbol to it. Pre-change behavior
/// dropped both.
fn write_scip_with_external_reference(
    path: &std::path::Path,
    source_root: &std::path::Path,
    workspace_sym: &str,
    workspace_rel_path: &str,
    external_sym: &str,
) {
    use protobuf::Message;
    use scip::types::{
        symbol_information, Document, Index, Metadata, Occurrence, SymbolInformation, ToolInfo,
    };
    let mut idx = Index::new();
    let mut md = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "test".into();
    tool.version = "0.0.0".into();
    md.tool_info = protobuf::MessageField::some(tool);
    md.project_root = format!("file://{}", source_root.display());
    idx.metadata = protobuf::MessageField::some(md);

    let mut doc = Document::new();
    doc.language = "rust".into();
    doc.relative_path = workspace_rel_path.into();

    // SymbolInformation for the workspace symbol (function).
    let mut sym = SymbolInformation::new();
    sym.symbol = workspace_sym.into();
    sym.kind = protobuf::EnumOrUnknown::new(symbol_information::Kind::Function);
    doc.symbols.push(sym);

    // Definition occurrence for the workspace symbol — this is what
    // populates `def_counts[workspace_sym] = 1` in pass 1, so pass 2
    // treats it as locally defined. The 4-component range
    // [start_line, start_col, end_line, end_col] spans the whole
    // function body so the enclosing-symbol attribution at the
    // reference site (below) lands inside it.
    let mut def_occ = Occurrence::new();
    def_occ.range = vec![0_i32, 0, 10, 0];
    def_occ.symbol = workspace_sym.into();
    def_occ.symbol_roles = scip::types::SymbolRole::Definition as i32;
    doc.occurrences.push(def_occ);

    // Reference occurrence inside the workspace symbol's body, pointing
    // at the external symbol. No matching SymbolInformation and no
    // Definition occurrence anywhere → def_counts[external_sym] == 0.
    let mut ref_occ = Occurrence::new();
    ref_occ.range = vec![1_i32, 4, 1, 14];
    ref_occ.symbol = external_sym.into();
    ref_occ.symbol_roles = 0; // Plain reference.
    doc.occurrences.push(ref_occ);

    idx.documents.push(doc);
    let bytes = idx.write_to_bytes().expect("write_to_bytes");
    std::fs::write(path, bytes).expect("write scip file");
}

/// Task §3.1 — full pipeline end-to-end on a synthetic SCIP file that
/// references an external symbol. Asserts:
/// - the external symbol appears in the symbols table with `external = true`
/// - a `calls` edge links the workspace symbol to it
/// Pre-change, both assertions would fail (the symbol wouldn't be
/// emitted, and the edge would be dropped by the `def_count == 0` arm).
#[tokio::test(flavor = "current_thread")]
async fn external_symbol_lands_with_external_true_and_inbound_edge() {
    use kenn_store::api::Reader as _;
    use kenn_store::open_reader;

    let dir = TempDir::new().unwrap();
    let source_root = dir.path().join("ws");
    let src = source_root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "fn caller() { let _ = result.unwrap(); }\n",
    )
    .unwrap();
    let snapshot = source_root.join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&snapshot).unwrap();

    let scip_path = dir.path().join("synthetic.scip");
    let workspace_sym = "rust-analyzer cargo k 0.1 m/caller().";
    let external_sym = "rust-analyzer cargo core 0.0 result/Result#unwrap().";
    // The Workspace canonicalizes its root (resolves macOS /var → /private/var
    // symlinks), so the SCIP project_root must use the same resolved form or
    // canonicalize's strip_prefix will fail with OutsideRoot.
    let canon_root = source_root
        .canonicalize()
        .expect("canonicalize source_root");
    write_scip_with_external_reference(
        &scip_path,
        &canon_root,
        workspace_sym,
        "src/lib.rs",
        external_sym,
    );

    let writer = open_writer(&snapshot, WriterOptions::default())
        .await
        .expect("open_writer");

    let workspace = Workspace::new(&source_root, &[]).expect("workspace");
    let runner = IndexerDriver::new(workspace).with_scip_driver(SyntheticScipDriver {
        language: "rust",
        scip_path,
    });

    let (reports, writer) = tokio::task::spawn_blocking(move || {
        run_pipeline_with_progress(
            &runner,
            writer,
            16,
            |_| {},
            kenn_indexer::pipeline::no_op_hook(),
            None,
        )
    })
    .await
    .expect("spawn_blocking")
    .expect("run_pipeline");
    assert!(
        reports
            .iter()
            .all(|r| !matches!(r.status, kenn_indexer::report::RunStatus::Failed)),
        "pipeline reports must not be Failed: {reports:?}"
    );

    // `run_pipeline_with_progress` already finalized the writer (phase 4); just
    // release its connections before opening the snapshot read-only.
    drop(writer);

    let reader = open_reader(&snapshot).await.expect("open_reader");
    let symbols = reader.scan_symbols().await.expect("scan_symbols");

    // Find the external symbol row.
    let external_rows: Vec<_> = symbols.iter().filter(|s| s.external).collect();
    assert!(
        external_rows.iter().any(|s| s.name == "unwrap"),
        "expected an external symbol named 'unwrap', got: {:?}",
        symbols
            .iter()
            .map(|s| (&s.name, s.external))
            .collect::<Vec<_>>(),
    );

    // Task §3.2 partial — verify find_symbol_tiered with the filter.
    let found_with = reader
        .find_symbol_tiered("unwrap", 10, true, true)
        .await
        .expect("find_symbol with external");
    assert!(
        !found_with.is_empty(),
        "find_symbol_tiered('unwrap', include_external=true) MUST surface the external symbol"
    );
    let found_without = reader
        .find_symbol_tiered("unwrap", 10, false, true)
        .await
        .expect("find_symbol without external");
    assert!(
        found_without.is_empty(),
        "find_symbol_tiered('unwrap', include_external=false) MUST exclude the external symbol; got: {found_without:?}",
    );
}

/// Drive `run_pipeline_with_progress` with a stub SCIP driver pointing
/// at a synthetic .scip file on disk. Exercises `ingest_scip_driver`
/// — the SCIP-path counterpart to `ingest_jsonl_into_sink`.
#[test]
fn run_pipeline_drives_ingest_scip_driver_against_synthetic_scip() {
    let dir = TempDir::new().unwrap();
    let source_root = dir.path().join("ws");
    let src = source_root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "// fixture\n").unwrap();
    let building = source_root.join(".kenn").join("local").join("building");
    std::fs::create_dir_all(&building).unwrap();

    let scip_path = dir.path().join("synthetic.scip");
    write_synthetic_scip(
        &scip_path,
        "rust",
        "src/lib.rs",
        "rust-analyzer cargo k 0.1 m/Foo#",
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    let writer = rt
        .block_on(open_writer(&building, WriterOptions::default()))
        .expect("open_writer");

    let workspace = Workspace::new(&source_root, &[]).expect("workspace");
    let runner = IndexerDriver::new(workspace).with_scip_driver(SyntheticScipDriver {
        language: "rust",
        scip_path,
    });

    // `run_pipeline_with_progress` takes the writer directly; it
    // builds per-driver BatchSinks internally.
    let (reports, _writer) = run_pipeline_with_progress(
        &runner,
        writer,
        16,
        |_| {},
        kenn_indexer::pipeline::no_op_hook(),
        None,
    )
    .expect("run_pipeline");
    assert!(
        !reports.is_empty(),
        "stub driver produced at least one report"
    );
    assert!(
        reports.iter().any(|r| r.indexer_name == "rust"),
        "rust language id present in reports"
    );
    // The SCIP `tool_info.version` the fixture writes replaces the `"?"` the
    // driver seeded. SCIP producers must reach the report the same way JSONL
    // ones do — this was fixed for JSONL first, leaving rust/go/python at "?".
    let rust = reports
        .iter()
        .find(|r| r.indexer_name == "rust")
        .expect("a rust report");
    assert_eq!(
        rust.indexer_version, "0.0.0",
        "the SCIP tool_info version reaches the run report"
    );
}
