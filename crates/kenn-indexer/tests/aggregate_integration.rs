//! End-to-end aggregation: write a tiny corpus through `DbWriter`,
//! run the aggregation pass that the pipeline would run, `finalize`, then
//! reopen via the reader and verify both `aggregate_nodes` and
//! `aggregate_edges` carry the rolled-up graph.
//!
//! Covers task 4.4 (integration), 4.5 (determinism: same input twice
//! produces byte-identical scans), 4.6 (partial ingest: missing data
//! still aggregates what's there).

use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, PackageRecord, SymbolRecord,
};
use kenn_store::api::{Reader, WriteBatch};
use kenn_store::{open_reader, open_writer, WriterOptions};
use tempfile::TempDir;

fn pkg() -> PackageRecord {
    PackageRecord {
        id: 1,
        name: "alpha-pkg".into(),
        version: "0".into(),
        manager: "cargo".into(),
        external: false,
    }
}

fn file(id: u32, path: &str) -> FileRecord {
    FileRecord {
        id,
        path: path.into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 0,
    }
}

fn sym(id: u32, kind: Kind, name: &str, enclosing: u32, pkg_id: u32) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("rs:{name}"),
        language: Language::Rust,
        pkg_id,
        kind,
        name: name.into(),
        enclosing_sym_id: enclosing,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn def(sym_id: u32, file_id: u32) -> DefRecord {
    DefRecord {
        sym_id,
        file_id,
        start_line: 1,
        start_col: 0,
        end_line: 1,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

fn calls(src: u32, tgt: u32) -> EdgeRecord {
    EdgeRecord {
        src_id: src,
        target_id: tgt,
        properties: EdgeProperties::Calls,
    }
}

/// Corpus:
///   pkg 1 "alpha-pkg"
///   file 10 "crates/alpha/src/lib.rs"
///   symbol 100 Class Foo (pkg=1)
///   symbol 101 Method `foo_method` (pkg=1, enclosing=100)
///   symbol 200 Class Bar (pkg=1)
///   symbol 201 Method `bar_method` (pkg=1, enclosing=200)
///   defs:   100→10, 101→10, 200→10, 201→10
///   edges:  Calls(101, 201) + Calls(101, 201) again (different sources
///           reduce-by-pair below — we only have one edge but we want
///           the test to be explicit; aggregation result: one edge
///           Calls between aggregates 100 and 200 with weight 3).
fn build_corpus() -> WriteBatch {
    WriteBatch {
        files: vec![file(10, "crates/alpha/src/lib.rs")],
        packages: vec![pkg()],
        symbols: vec![
            sym(100, Kind::Class, "Foo", 0, 1),
            sym(101, Kind::Method, "foo_method", 100, 1),
            sym(200, Kind::Class, "Bar", 0, 1),
            sym(201, Kind::Method, "bar_method", 200, 1),
        ],
        symbol_docs: vec![],
        file_docs: vec![],
        defs: vec![def(100, 10), def(101, 10), def(200, 10), def(201, 10)],
        edges: vec![calls(101, 201)],
    }
}

async fn write_corpus_and_aggregate(dir: &std::path::Path) {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    writer
        .write_batch(&build_corpus())
        .await
        .expect("write_batch");
    // Pipeline equivalent: aggregate, then finalize.
    let out = kenn_indexer::aggregate::compute_and_persist(
        &writer,
        &kenn_indexer::package_layout::PackageLayout::empty(),
        kenn_indexer::pipeline::no_op_hook(),
        None,
    )
    .await
    .expect("compute_and_persist");
    assert!(out.is_some(), "default backend should aggregate");
    writer.finalize().await.expect("finalize");
}

#[tokio::test(flavor = "current_thread")]
async fn pipeline_persists_aggregate_tables() {
    let tmp = TempDir::new().unwrap();
    let snapshot = tmp.path().join("snapshot");
    write_corpus_and_aggregate(&snapshot).await;

    let reader = open_reader(&snapshot).await.unwrap();
    let nodes = reader.scan_aggregate_nodes().await.unwrap();
    let edges = reader.scan_aggregate_edges().await.unwrap();

    assert!(!nodes.is_empty(), "expected at least one aggregate node");
    assert!(!edges.is_empty(), "expected at least one aggregate edge");

    // Two participating class aggregates: Foo (100) and Bar (200).
    let mut sids: Vec<u32> = nodes.iter().map(|n| n.id).collect();
    sids.sort_unstable();
    assert_eq!(sids, vec![100, 200]);
    for n in &nodes {
        assert_eq!(n.anchor_name, "alpha-pkg");
    }

    // One Calls edge between (100, 200) with weight 3.
    assert_eq!(edges.len(), 1);
    let e = &edges[0];
    assert_eq!(
        (e.src_id, e.dst_id, e.kind.db_name(), e.weight),
        (100, 200, "calls", 3)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn determinism_same_corpus_twice_byte_identical() {
    let tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();
    write_corpus_and_aggregate(&tmp_a.path().join("snap")).await;
    write_corpus_and_aggregate(&tmp_b.path().join("snap")).await;

    let ra = open_reader(&tmp_a.path().join("snap")).await.unwrap();
    let rb = open_reader(&tmp_b.path().join("snap")).await.unwrap();
    let na = ra.scan_aggregate_nodes().await.unwrap();
    let nb = rb.scan_aggregate_nodes().await.unwrap();
    let ea = ra.scan_aggregate_edges().await.unwrap();
    let eb = rb.scan_aggregate_edges().await.unwrap();

    // Sort defensively (iteration order is btree-sorted in redb, so the
    // table is already deterministic — assert it).
    let mut na2 = na;
    na2.sort_by_key(|n| n.id);
    let mut nb2 = nb;
    nb2.sort_by_key(|n| n.id);
    assert_eq!(na2.len(), nb2.len());
    for (a, b) in na2.iter().zip(nb2.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, b.name);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.anchor_id, b.anchor_id);
        assert_eq!(a.anchor_name, b.anchor_name);
    }
    assert_eq!(ea.len(), eb.len());
    for (a, b) in ea.iter().zip(eb.iter()) {
        assert_eq!(
            (a.src_id, a.dst_id, a.weight, a.kind.db_name()),
            (b.src_id, b.dst_id, b.weight, b.kind.db_name())
        );
    }
}

/// Partial-ingest tolerance: feed the writer ONLY the first class +
/// its method (no Bar, no edges). Aggregation must still run cleanly
/// and produce no edges (no inter-aggregate links left) but the snapshot
/// stays publishable.
#[tokio::test(flavor = "current_thread")]
async fn partial_corpus_still_aggregates_what_arrived() {
    let tmp = TempDir::new().unwrap();
    let snapshot = tmp.path().join("snapshot");
    let writer = open_writer(&snapshot, WriterOptions::default())
        .await
        .unwrap();
    // Only Foo + its method survive; no Bar, no edges.
    let partial = WriteBatch {
        files: vec![file(10, "crates/alpha/src/lib.rs")],
        packages: vec![pkg()],
        symbols: vec![
            sym(100, Kind::Class, "Foo", 0, 1),
            sym(101, Kind::Method, "foo_method", 100, 1),
        ],
        symbol_docs: vec![],
        file_docs: vec![],
        defs: vec![def(100, 10), def(101, 10)],
        edges: vec![],
    };
    writer.write_batch(&partial).await.unwrap();
    let out = kenn_indexer::aggregate::compute_and_persist(
        &writer,
        &kenn_indexer::package_layout::PackageLayout::empty(),
        kenn_indexer::pipeline::no_op_hook(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        Some((0, 0)),
        "no surviving edges → empty aggregate tables"
    );
    writer.finalize().await.unwrap();
    drop(writer);

    // Snapshot is still readable and the aggregate-graph tables exist
    // but are empty (the "published, but no aggregate edges yet"
    // condition the spec calls out for partial runs).
    let reader = open_reader(&snapshot).await.unwrap();
    assert!(reader.scan_aggregate_nodes().await.unwrap().is_empty());
    assert!(reader.scan_aggregate_edges().await.unwrap().is_empty());
}

/// Task 9.4 — `aggregate_edges` holds exactly one row per
/// `(node_min, node_max, kind)` after **symmetric** edge writes. Method
/// 101 (in class Foo) and method 201 (in class Bar) call each other both
/// ways; both directions roll up to the aggregate pair `(Foo, Bar)`, and
/// the aggregation pass canonicalizes endpoints + merges weights into
/// one row (design D5).
#[tokio::test(flavor = "multi_thread")]
async fn aggregate_edges_dedups_symmetric_writes() {
    let tmp = TempDir::new().unwrap();
    let snapshot = tmp.path().join("snapshot");
    let writer = open_writer(&snapshot, WriterOptions::default())
        .await
        .expect("open_writer");

    let batch = WriteBatch {
        files: vec![file(10, "src/lib.rs")],
        packages: vec![pkg()],
        symbols: vec![
            sym(100, Kind::Class, "Foo", 0, 1),
            sym(101, Kind::Method, "foo_method", 100, 1),
            sym(200, Kind::Class, "Bar", 0, 1),
            sym(201, Kind::Method, "bar_method", 200, 1),
        ],
        symbol_docs: vec![],
        file_docs: vec![],
        defs: vec![def(100, 10), def(101, 10), def(200, 10), def(201, 10)],
        // Symmetric: 101 → 201 and 201 → 101.
        edges: vec![calls(101, 201), calls(201, 101)],
    };
    writer.write_batch(&batch).await.expect("write_batch");
    kenn_indexer::aggregate::compute_and_persist(
        &writer,
        &kenn_indexer::package_layout::PackageLayout::empty(),
        kenn_indexer::pipeline::no_op_hook(),
        None,
    )
    .await
    .expect("compute_and_persist");
    drop(writer);

    let reader = open_reader(&snapshot).await.expect("open_reader");
    let edges = reader.scan_aggregate_edges().await.expect("scan edges");
    let mut calls_rows: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == kenn_model::EdgeKind::Calls)
        .collect();
    calls_rows.sort_by_key(|e| (e.src_id, e.dst_id));
    // Directed: the symmetric method calls roll up to TWO directed class edges,
    // Foo → Bar and Bar → Foo — not collapsed to one canonicalized undirected row.
    assert_eq!(
        calls_rows.len(),
        2,
        "symmetric writes yield two directed aggregate edges; got {calls_rows:?}"
    );
    assert_eq!((calls_rows[0].src_id, calls_rows[0].dst_id), (100, 200));
    assert_eq!((calls_rows[1].src_id, calls_rows[1].dst_id), (200, 100));
    assert!(
        calls_rows.iter().all(|e| e.weight == 3),
        "each direction keeps its own weight (no merge)"
    );
}

/// Two packages, four classes (two each), cross-package method calls; `Foo` is
/// the hub (heaviest weighted degree). A fake analysis hook puts all four class
/// aggregates in one cross-package flat community — what real flat-Louvain would
/// produce for a dense cluster spanning two packages. Runs the pipeline into
/// `root/atlas` and returns that dir. Shared by the domain assertion +
/// determinism tests; a fixed timestamp is the only wall-clock value, so the
/// bundle is reproducible.
#[expect(
    clippy::too_many_lines,
    reason = "one linear fixture: records, then the pipeline run that consumes them; splitting only hides what the test feeds in"
)]
async fn build_cross_package_domain_atlas(root: &std::path::Path) -> std::path::PathBuf {
    use kenn_model::{AnalysisFlatCommunityRecord, AnalysisNodeMembershipRecord};

    let snapshot = root.join("snapshot");
    let out_dir = root.join("atlas");
    let writer = open_writer(&snapshot, WriterOptions::default())
        .await
        .expect("open_writer");
    let batch = WriteBatch {
        files: vec![
            file(10, "crates/alpha/src/lib.rs"),
            file(20, "crates/beta/src/lib.rs"),
        ],
        packages: vec![
            PackageRecord {
                id: 1,
                name: "alpha".into(),
                version: "0".into(),
                manager: "cargo".into(),
                external: false,
            },
            PackageRecord {
                id: 2,
                name: "beta".into(),
                version: "0".into(),
                manager: "cargo".into(),
                external: false,
            },
        ],
        symbols: vec![
            sym(100, Kind::Class, "Foo", 0, 1),
            sym(101, Kind::Method, "foo_m", 100, 1),
            sym(200, Kind::Class, "Bar", 0, 1),
            sym(201, Kind::Method, "bar_m", 200, 1),
            sym(300, Kind::Class, "Baz", 0, 2),
            sym(301, Kind::Method, "baz_m", 300, 2),
            sym(400, Kind::Class, "Qux", 0, 2),
            sym(401, Kind::Method, "qux_m", 400, 2),
        ],
        symbol_docs: vec![],
        file_docs: vec![],
        defs: vec![
            def(100, 10),
            def(101, 10),
            def(200, 10),
            def(201, 10),
            def(300, 20),
            def(301, 20),
            def(400, 20),
            def(401, 20),
        ],
        edges: vec![
            calls(101, 301), // Foo → Baz (cross-package)
            calls(101, 201), // Foo → Bar
            calls(101, 401), // Foo → Qux (cross-package)
            calls(201, 301), // Bar → Baz (cross-package)
        ],
    };
    writer.write_batch(&batch).await.expect("write_batch");

    let community = |short_id| AnalysisNodeMembershipRecord {
        short_id,
        flat_community_id: 1,
        anchored_leaf_community_id: 0,
    };
    let hook: kenn_indexer::pipeline::PostAggregateHook = Box::new(move |_n, _e, w| {
        Box::pin(async move {
            let flat = vec![AnalysisFlatCommunityRecord {
                community_id: 1,
                size: 4,
                total_weight: 8,
                cross_anchor: true,
                primary_anchor_id: 1,
                primary_anchor_name: "alpha".into(),
            }];
            let membership = vec![
                community(100),
                community(200),
                community(300),
                community(400),
            ];
            w.write_analysis_tables(&[], &flat, &[], &membership).await
        })
    });
    let atlas = kenn_indexer::atlas::producer::AtlasContext {
        out_dir: out_dir.clone(),
        source_root: root.to_path_buf(),
        // No committed store in this fixture — nothing to hang a pointer off.
        pointer_dir: None,
        workspace_name: "ws".into(),
        freshness: "HEAD abc".into(),
        timestamp: "2026-07-15T00:00:00Z".into(),
    };
    kenn_indexer::aggregate::compute_and_persist(
        &writer,
        &kenn_indexer::package_layout::PackageLayout::empty(),
        hook,
        Some(&atlas),
    )
    .await
    .expect("compute_and_persist")
    .expect("aggregated");
    out_dir
}

/// The `domains` axis, end to end: the atlas reads the flat community the hook
/// wrote back and emits a domain concept — proving the reorder + writer
/// read-back wiring without depending on `kenn-analyze`.
#[tokio::test(flavor = "current_thread")]
async fn atlas_emits_a_cross_package_domain() {
    let tmp = TempDir::new().unwrap();
    let out_dir = build_cross_package_domain_atlas(tmp.path()).await;

    let index = std::fs::read_to_string(out_dir.join("index.md")).expect("index.md");
    assert!(
        index.contains("## Domains"),
        "index lists a Domains section:\n{index}"
    );
    assert!(
        index.contains("1 domains"),
        "header counts the domain:\n{index}"
    );
    assert!(
        index.contains("[Foo](/domains/Foo.md)"),
        "domain linked by its hub symbol:\n{index}"
    );

    let doc = std::fs::read_to_string(out_dir.join("domains/Foo.md")).expect("domain doc");
    assert!(doc.contains("type: domain"), "domain concept type:\n{doc}");
    assert!(
        doc.contains("## Spanned packages"),
        "spanned-packages section:\n{doc}"
    );
    assert!(
        doc.contains("(/packages/rust_alpha.md)") && doc.contains("(/packages/rust_beta.md)"),
        "domain spans both packages (language-prefixed concept ids):\n{doc}"
    );
}

/// Determinism: the same corpus through the pipeline twice yields a
/// byte-identical domain bundle. Guards against `HashMap`-iteration-order
/// leaking into domain ordering, hub selection, or spanned-package lists (each
/// run builds fresh maps with distinct hasher seeds).
#[tokio::test(flavor = "current_thread")]
async fn domain_bundle_is_deterministic() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    let da = build_cross_package_domain_atlas(a.path()).await;
    let db = build_cross_package_domain_atlas(b.path()).await;

    let read = |dir: &std::path::Path, rel: &str| std::fs::read_to_string(dir.join(rel)).unwrap();
    assert_eq!(
        read(&da, "domains/Foo.md"),
        read(&db, "domains/Foo.md"),
        "domain doc must be byte-identical across runs"
    );
    // The Domains section of index.md (everything from the heading on) must match
    // too — ordering + glosses are order-independent.
    let domains_section = |s: String| s.split_once("## Domains").unwrap().1.to_string();
    assert_eq!(
        domains_section(read(&da, "index.md")),
        domains_section(read(&db, "index.md")),
        "index Domains section must be byte-identical across runs"
    );
}
