//! End-to-end test of the wired `SQLite` backend through the public factories
//! (`open_writer` → ingest → `finalize` → `open_reader` → query) — the exact
//! path `kenn-indexer` writes and `kenn-mcp` reads.

use kenn_store::api::{Reader, WriteBatch, WriterOptions};
use kenn_store::{open_reader, open_writer};

use kenn_model::{
    EdgeProperties, EdgeRecord, FileRecord, Kind, Language, SymbolDocsRecord, SymbolRecord,
};

fn sym(id: u32, name: &str) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("rs:{name}"),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: name.into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

#[tokio::test]
async fn open_writer_ingest_finalize_open_reader_query() {
    let dir = tempfile::tempdir().unwrap();

    // Write path (what kenn-indexer does).
    let writer = open_writer(dir.path(), WriterOptions::default())
        .await
        .expect("open_writer");
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: 1,
        path: "src/parser.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(sym(1, "parseUser"));
    b.symbols.push(sym(2, "helper"));
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 2,
        sig: "fn helper()".into(),
        doc: "parse the input".into(),
    });
    b.edges.push(EdgeRecord {
        src_id: 1,
        target_id: 2,
        properties: EdgeProperties::Calls,
    });
    writer.write_batch(&b).await.expect("write_batch");
    writer.finalize().await.expect("finalize");
    drop(writer); // close the writer's connections before reopening read-only

    // Read path (what kenn-mcp does).
    let reader = open_reader(dir.path()).await.expect("open_reader");

    // fetch
    let s = Reader::fetch_symbol_by_short_id(&reader, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.name, "parseUser");

    // identifier search
    let (hits, _) = Reader::search_symbols_by_name(&reader, "parse", 10, None, None, false, false)
        .await
        .unwrap();
    assert!(hits.iter().any(|h| h.name == "parseUser"), "{hits:?}");

    // blended search surfaces the doc-only match too
    let blended = Reader::search_symbols_blended(&reader, "parse", None, 10, false, false)
        .await
        .unwrap();
    let names: Vec<&str> = blended.iter().map(|h| h.symbol.name.as_str()).collect();
    assert!(
        names.contains(&"parseUser") && names.contains(&"helper"),
        "{names:?}"
    );

    // graph traversal
    let (out, total) = Reader::list_outbound(&reader, 1, "calls", 10, None, false, false)
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(out[0].id, 2);

    // catalog
    assert_eq!(Reader::count_table(&reader, "symbols").await.unwrap(), 2);
    assert_eq!(
        Reader::distinct_languages(&reader).await.unwrap(),
        vec!["rust".to_string()]
    );
}
