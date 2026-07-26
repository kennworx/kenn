use rusqlite::Connection;

use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisAnchoredCommunityRecord,
    AnalysisGodNodeRecord, AnalysisNodeMembershipRecord, EdgeKind, EdgeProperties,
};

use crate::api::types::WriterOptions;
use crate::api::WriteBatch;

use super::core::SqliteWriter;

use kenn_model::{
    DefRecord, EdgeRecord, FileDocsRecord, FileRecord, Kind, Language, PackageRecord,
    SymbolDocsRecord, SymbolRecord,
};
use tempfile::TempDir;

fn count(c: &Connection, t: &str) -> i64 {
    c.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn write_batch_populates_every_graph_table() {
    let dir = TempDir::new().unwrap();
    let w = SqliteWriter::create(dir.path(), crate::api::types::WriterOptions::default())
        .expect("create snapshot dbs");

    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: 1,
        path: "src/a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 0xDEAD_BEEF_CAFE,
    });
    b.packages.push(PackageRecord {
        id: 1,
        name: "kenn".into(),
        version: "0".into(),
        manager: "cargo".into(),
        external: false,
    });
    b.symbols.push(SymbolRecord {
        id: 1,
        pub_id: "rs:foo".into(),
        language: Language::Rust,
        pkg_id: 1,
        kind: Kind::Function,
        name: "Foo".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 2,
        targs: 0,
        external: false,
        test: false,
    });
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: "fn foo()".into(),
        doc: "docs".into(),
    });
    b.file_docs.push(FileDocsRecord {
        file_id: 1,
        doc: "file-level doc".into(),
    });
    b.defs.push(DefRecord {
        sym_id: 1,
        file_id: 1,
        start_line: 1,
        start_col: 0,
        end_line: 2,
        end_col: 1,
        body_start_line: 0,
        body_end_line: 0,
    });
    b.edges.push(EdgeRecord {
        src_id: 1,
        target_id: 1,
        properties: EdgeProperties::Calls,
    });

    w.write_batch(&b).expect("write_batch");

    for (t, n) in [
        ("files", 1),
        ("packages", 1),
        ("symbols", 1),
        ("symbol_docs", 1),
        ("file_docs", 1),
        ("defs", 1),
        ("edges", 1),
    ] {
        assert_eq!(count(w.graph(), t), n, "row count for {t}");
    }
    // name_lower is derived; u64 content_hash round-trips bit-preserved.
    let nl: String = w
        .graph()
        .query_row("SELECT name_lower FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nl, "foo");
    let ch: i64 = w
        .graph()
        .query_row("SELECT content_hash FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(u64::from_le_bytes(ch.to_le_bytes()), 0xDEAD_BEEF_CAFE);
}

#[test]
fn finalize_drops_placeholder_defs_that_shadow_a_real_def() {
    // The per-document SCIP/JSONL transform emits a [0,0,0,0] placeholder for a
    // symbol that appears in a document but is defined elsewhere. finalize must
    // drop such placeholders when the symbol also has a real def, and keep them
    // when it is the symbol's only def (truly synthetic / defined nowhere).
    let dir = TempDir::new().unwrap();
    let w = SqliteWriter::create(dir.path(), WriterOptions::default()).unwrap();

    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: 1,
        path: "src/a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    // sym 1: a real def (line 19) plus a spurious zero-range placeholder.
    b.symbols.push(SymbolRecord {
        id: 1,
        pub_id: "rs:defined".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Struct,
        name: "Defined".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: 1,
        file_id: 1,
        start_line: 19,
        start_col: 0,
        end_line: 19,
        end_col: 5,
        body_start_line: 0,
        body_end_line: 0,
    });
    b.defs.push(DefRecord {
        sym_id: 1,
        file_id: 1,
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    // sym 2: only the placeholder — must survive so the symbol stays addressable.
    b.symbols.push(SymbolRecord {
        id: 2,
        pub_id: "rs:synthetic".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Struct,
        name: "Synthetic".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: 2,
        file_id: 1,
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });

    w.write_batch(&b).unwrap();
    w.finalize().unwrap();

    // sym 1: the shadowing placeholder is gone; only the real def survives.
    let sym1: Vec<u32> = {
        let mut stmt = w
            .graph()
            .prepare("SELECT start_line FROM defs WHERE sym_id = 1 ORDER BY start_line")
            .unwrap();
        let out = stmt
            .query_map([], |r| r.get::<_, u32>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out
    };
    assert_eq!(
        sym1,
        vec![19],
        "placeholder shadowing a real def is dropped"
    );

    // sym 2: the sole placeholder is kept.
    let sym2: i64 = w
        .graph()
        .query_row("SELECT count(*) FROM defs WHERE sym_id = 2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(sym2, 1, "a synthetic-only placeholder is kept");
}

#[test]
fn markdown_link_edges_persist_grade_and_relation() {
    use kenn_model::{EdgeRecord, LinkGrade};
    let dir = TempDir::new().unwrap();
    let w = SqliteWriter::create(dir.path(), WriterOptions::default()).unwrap();
    let mut b = WriteBatch::default();
    b.edges.push(EdgeRecord {
        src_id: 10,
        target_id: 20,
        properties: EdgeProperties::LinksTo {
            grade: LinkGrade::Drifted,
            relation: "extends".into(),
        },
    });
    b.edges.push(EdgeRecord {
        src_id: 11,
        target_id: 21,
        properties: EdgeProperties::Embeds {
            grade: LinkGrade::Exact,
        },
    });
    w.write_batch(&b).unwrap();

    // links_to: kind code 12, link_grade = drifted(1), relation = "extends".
    let (kind, grade, relation): (u32, Option<u8>, Option<String>) = w
        .graph()
        .query_row(
            "SELECT kind, link_grade, link_relation FROM edges WHERE src_id=10",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, 12);
    assert_eq!(grade, Some(1));
    assert_eq!(relation.as_deref(), Some("extends"));

    // embeds: kind code 13, link_grade = exact(0), no relation.
    let (kind, grade, relation): (u32, Option<u8>, Option<String>) = w
        .graph()
        .query_row(
            "SELECT kind, link_grade, link_relation FROM edges WHERE src_id=11",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, 13);
    assert_eq!(grade, Some(0));
    assert_eq!(relation, None);
}

#[test]
fn finalize_builds_knowledge_rows_and_fts() {
    let dir = TempDir::new().unwrap();
    let w = SqliteWriter::create(dir.path(), crate::api::types::WriterOptions::default()).unwrap();
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: 1,
        path: "src/a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.file_docs.push(FileDocsRecord {
        file_id: 1,
        doc: "crate level docs".into(),
    });
    b.symbols.push(SymbolRecord {
        id: 1,
        pub_id: "rs:foo".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "parseUser".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: "fn parse_user()".into(),
        doc: "parses a user".into(),
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();

    let k = w.knowledge();
    // 1 name row + 1 symbol-doc row + 1 file-doc row.
    assert_eq!(count(k, "knowledge"), 3);
    assert_eq!(count(k, "name_fts"), 1);
    assert_eq!(count(k, "doc_fts"), 2);
    // Trigram FTS finds the split identifier (`parse_user` → "parse user").
    let hits: i64 = k
        .query_row(
            "SELECT count(*) FROM name_fts WHERE name_fts MATCH 'parse'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1);
}

#[test]
fn name_row_fingerprints_the_first_nonempty_doc() {
    use crate::db::codes::text_fingerprint;
    let dir = TempDir::new().unwrap();
    let w = SqliteWriter::create(dir.path(), WriterOptions::default()).unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(SymbolRecord {
        id: 1,
        pub_id: "rs:foo".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "foo".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    // Two doc rows for one symbol (the partial-type shape): the first is
    // sig-only with an empty doc, the second carries the real prose. The
    // doc-only fingerprint must track the real doc — the same one `scan_rows`
    // embeds — not `fp("")`.
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: "fn foo()".into(),
        doc: String::new(),
    });
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: "fn foo()".into(),
        doc: "the real doc".into(),
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();

    let fp: i64 = w
        .knowledge()
        .query_row(
            "SELECT fingerprint FROM knowledge WHERE row_kind='name'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        u64::from_le_bytes(fp.to_le_bytes()),
        text_fingerprint("the real doc"),
        "name row must fingerprint the first non-empty doc"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one self-contained fixture exercising the aggregate + analysis write/scan round-trip"
)]
fn aggregate_analysis_writes_and_scans_roundtrip() {
    use kenn_model::{DefRecord, EdgeRecord, GodNodeFilter};
    let dir = TempDir::new().unwrap();
    let w = SqliteWriter::create(dir.path(), crate::api::types::WriterOptions::default()).unwrap();
    let mut b = WriteBatch::default();
    b.packages.push(PackageRecord {
        id: 1,
        name: "kenn".into(),
        version: "0".into(),
        manager: "cargo".into(),
        external: false,
    });
    b.files.push(FileRecord {
        id: 7,
        path: "a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 9,
    });
    b.symbols.push(SymbolRecord {
        id: 1,
        pub_id: "rs:foo".into(),
        language: Language::Rust,
        pkg_id: 1,
        kind: Kind::Function,
        name: "Foo".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 2,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: 1,
        file_id: 7,
        start_line: 1,
        start_col: 0,
        end_line: 2,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    b.edges.push(EdgeRecord {
        src_id: 1,
        target_id: 1,
        properties: EdgeProperties::Calls,
    });
    w.write_batch(&b).unwrap();

    w.write_aggregate_tables(
        &[AggregateNodeRecord {
            id: 1,
            kind: Kind::Class,
            name: "C".into(),
            language: Language::Rust,
            external: false,
            test: false,
            example: false,
            anchor_id: 0,
            anchor_name: "<unanchored>".into(),
        }],
        &[AggregateEdgeRecord {
            src_id: 1,
            dst_id: 2,
            kind: EdgeKind::Calls,
            weight: 3,
        }],
    )
    .unwrap();
    w.write_analysis_tables(
        &[AnalysisGodNodeRecord {
            filter: GodNodeFilter::Live,
            rank: 0,
            short_id: 1,
            weighted_degree: 5,
            name: "C".into(),
            kind: Kind::Class,
            anchor_id: 0,
            anchor_name: "a".into(),
        }],
        &[],
        &[AnalysisAnchoredCommunityRecord {
            community_id: 0,
            parent_id: None,
            depth: 0,
            anchor_id: 0,
            anchor_name: "a".into(),
            size: 1,
            test_ratio: 0.0,
            test_infra: false,
        }],
        &[AnalysisNodeMembershipRecord {
            short_id: 1,
            flat_community_id: 0,
            anchored_leaf_community_id: 0,
        }],
    )
    .unwrap();

    // Scans reconstruct records, round-tripping the enums via from_db_name.
    let syms = w.scan_symbols_for_aggregation().unwrap();
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0].kind, Kind::Function);
    assert_eq!(syms[0].language, Language::Rust);
    assert_eq!(syms[0].nargs, 2);
    assert_eq!(w.scan_files_for_aggregation().unwrap()[0].content_hash, 9);
    assert_eq!(w.scan_packages_for_aggregation().unwrap()[0].name, "kenn");
    assert_eq!(w.scan_def_files_for_aggregation().unwrap(), vec![(1, 7)]);
    assert_eq!(
        w.scan_edges_for_aggregation(EdgeKind::Calls).unwrap(),
        vec![(1, 1)]
    );

    let n: i64 = w
        .graph()
        .query_row("SELECT count(*) FROM aggregate_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    let g: i64 = w
        .graph()
        .query_row("SELECT count(*) FROM analysis_god_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(g, 1);
}

#[test]
#[expect(
    clippy::many_single_char_names,
    reason = "terse local bindings (writer w, batch b, connection k, count n, vector v) in a self-contained fixture"
)]
fn finalize_reconciles_sidecar_vectors_into_vec0() {
    use crate::db::codes::text_fingerprint;
    use crate::embed::sidecar::io::{append_vectors, WriterPrefix};
    use crate::embed::sidecar::manifest::{Manifest, CODE_TEXT_RECIPE};
    use rusqlite::{Connection, OpenFlags};

    let root = TempDir::new().unwrap();
    let snapshot = root.path().join("snap");
    let sidecar = root.path().join("vectors");
    let tmp = sidecar.join(".tmp");
    std::fs::create_dir_all(&tmp).unwrap();

    // The committed vector for `parseUser`'s doc-only embeddable text, keyed by
    // the same xxh3 fingerprint (over the doc prose) that finalize uses.
    let doc = "parses a user from raw input";
    let fp = text_fingerprint(doc);
    let mut v = vec![0.1_f32; 768];
    v[0] = 1.0;
    append_vectors(&sidecar, &tmp, WriterPrefix::Seg, 768, &[(fp, v.clone())]).unwrap();
    Manifest::current("test-model".to_owned(), 768, CODE_TEXT_RECIPE)
        .write(&sidecar)
        .unwrap();

    let w = SqliteWriter::create(
        &snapshot,
        WriterOptions {
            vectors_dir: Some(sidecar.clone()),
            vectors_model_id: Some("test-model".to_owned()),
            ..WriterOptions::default()
        },
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(SymbolRecord {
        id: 1,
        pub_id: "rs:parseUser".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "parseUser".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: "fn parseUser()".into(),
        doc: doc.into(),
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    // The reconciled vector is in vec0, and a KNN query over it returns
    // the symbol — embeddings work on sqlite-vec, no model.
    super::super::ensure_vec_extension();
    let k =
        Connection::open_with_flags(snapshot.join("vector.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let n: i64 = k
        .query_row("SELECT count(*) FROM vec_knowledge", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "sidecar vector reconciled into vec0");

    let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    let pub_id: String = k
        .query_row(
            "SELECT kn.pub_id FROM vec_knowledge vk \
             JOIN knowledge kn ON kn.rowid = vk.rowid \
             WHERE vk.embedding MATCH ?1 AND vk.k = 1 ORDER BY distance",
            [bytes],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pub_id, "rs:parseUser");
}
