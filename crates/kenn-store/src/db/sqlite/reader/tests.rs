use super::projection::SqliteReader;
use crate::api::Reader;
use crate::api::RowNarrow;
use crate::api::WriteBatch;
use kenn_model::{
    EdgeKind, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, PackageRecord, SymbolRecord,
};
use tempfile::TempDir;

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
async fn finalize_builds_per_language_subset_and_manager_stats() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    // rust: 2 internal, 1 test, 1 external symbol.
    b.symbols.push(sym(1, "alpha"));
    b.symbols.push(sym(2, "beta"));
    let mut test_sym = sym(3, "alpha_test");
    test_sym.test = true;
    b.symbols.push(test_sym);
    let mut ext_sym = sym(4, "vendored");
    ext_sym.external = true;
    b.symbols.push(ext_sym);
    b.files.push(FileRecord {
        id: 7,
        path: "src/a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    // packages: one workspace (internal), one dependency (external), same manager.
    b.packages.push(PackageRecord {
        id: 1,
        name: "kenn".into(),
        version: "0".into(),
        manager: "cargo".into(),
        external: false,
    });
    b.packages.push(PackageRecord {
        id: 2,
        name: "serde".into(),
        version: "1".into(),
        manager: "cargo".into(),
        external: true,
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    let rows = r.stats().await.unwrap();
    let get = |scope: &str, key: &str, subset: &str, metric: &str| {
        rows.iter()
            .find(|s| s.scope == scope && s.key == key && s.subset == subset && s.metric == metric)
            .map(|s| s.value)
    };
    assert_eq!(get("language", "rust", "internal", "symbols"), Some(2));
    assert_eq!(get("language", "rust", "test", "symbols"), Some(1));
    assert_eq!(get("language", "rust", "external", "symbols"), Some(1));
    assert_eq!(get("language", "rust", "internal", "files"), Some(1));
    assert_eq!(get("manager", "cargo", "internal", "packages"), Some(1));
    assert_eq!(get("manager", "cargo", "external", "packages"), Some(1));
    // No grand-total `all` rows.
    assert!(!rows.iter().any(|s| s.subset == "all"));
}

#[tokio::test]
async fn code_node_resolver_matches_canonical_pub_id() {
    // Regression: the staleness resolver must key on the `pub_id` column, which
    // is already the canonical code-node id (it carries the `rs:`/`cs:`/… short
    // code — the form `find_symbol` returns and agents store in `parent_ids`).
    // A prior bug re-prefixed it with the `language` column (`rust:rs:foo`), so
    // `contains` never matched and every code-cited finding folded to stale.
    use crate::db::findings::CodeNodeResolver;
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "billing::Order")); // pub_id == "rs:billing::Order"
    w.write_batch(&b).unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.expect("open reader");
    let resolver = r.code_node_resolver().await.expect("resolver");
    // The canonical id resolves…
    assert!(resolver.contains("rs:billing::Order"));
    // …the language-doubled form (the bug) does not exist…
    assert!(!resolver.contains("rust:rs:billing::Order"));
    // …and a genuinely-absent symbol is correctly not contained.
    assert!(!resolver.contains("rs:billing::Missing"));
}

#[tokio::test]
async fn open_projects_symbols_files_and_catalog() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: 7,
        path: "src/a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: 42,
        pub_id: "rs:foo".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "Foo".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 3,
        targs: 0,
        external: false,
        test: false,
    });
    w.write_batch(&b).unwrap();
    drop(w); // close writer connections before reopening read-only

    let r = SqliteReader::open(dir.path()).await.expect("open reader");
    let s = r
        .fetch_symbol_by_short_id(42)
        .await
        .unwrap()
        .expect("symbol 42");
    assert_eq!(s.pub_id, "rs:foo");
    assert_eq!(s.name, "Foo");
    assert_eq!(s.nargs, 3);
    assert_eq!(
        r.fetch_file_path(7).await.unwrap().as_deref(),
        Some("src/a.rs")
    );
    assert_eq!(r.fetch_file_short_id("src/a.rs").await.unwrap(), Some(7));
    assert_eq!(
        r.distinct_languages().await.unwrap(),
        vec!["rust".to_string()]
    );
    assert_eq!(r.count_table("symbols").await.unwrap(), 1);
    assert_eq!(r.count_table("files").await.unwrap(), 1);
    assert_eq!(r.count_table("nonexistent").await.unwrap(), 0);
}

#[tokio::test]
async fn traversal_walks_the_csr_both_directions() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "caller"));
    b.symbols.push(sym(2, "callee"));
    b.edges.push(EdgeRecord {
        src_id: 1,
        target_id: 2,
        properties: EdgeProperties::Calls,
    });
    w.write_batch(&b).unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    let calls = EdgeKind::Calls.db_name();

    let (out, total) = r
        .list_outbound(1, calls, 10, None, &RowNarrow::visibility(false, false))
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, 2);
    assert_eq!(out[0].name, "callee");

    let (inb, itotal) = r
        .list_inbound(2, calls, 10, None, &RowNarrow::visibility(false, false))
        .await
        .unwrap();
    assert_eq!(itotal, 1);
    assert_eq!(inb[0].id, 1);

    // No inbound calls to the caller; unknown relation errors.
    assert_eq!(
        r.list_inbound(1, calls, 10, None, &RowNarrow::visibility(false, false))
            .await
            .unwrap()
            .1,
        0
    );
    r.list_outbound(1, "bogus", 10, None, &RowNarrow::visibility(false, false))
        .await
        .unwrap_err();
}

#[tokio::test]
async fn fts_identifier_search_finds_and_ranks() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "parseUser"));
    b.symbols.push(sym(2, "parser"));
    b.symbols.push(sym(3, "unrelated"));
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    let (hits, _) = r
        .search_symbols_by_name("parse", 10, None, None, false, false)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert!(names.contains(&"parseUser"), "got {names:?}");
    assert!(names.contains(&"parser"), "got {names:?}");
    assert!(!names.contains(&"unrelated"), "got {names:?}");

    // Exact whole-name match is boosted to the top.
    let (exact, _) = r
        .search_symbols_by_name("parser", 10, None, None, false, false)
        .await
        .unwrap();
    assert_eq!(exact.first().map(|h| h.name.as_str()), Some("parser"));

    // Sub-trigram queries return nothing.
    let (short, _) = r
        .search_symbols_by_name("pa", 10, None, None, false, false)
        .await
        .unwrap();
    assert!(short.is_empty());
}

#[tokio::test]
async fn fetch_defs_docs_packages_and_location() {
    use kenn_model::{DefRecord, FileRecord, PackageRecord, SymbolDocsRecord};
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
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
        path: "src/a.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    let mut s = sym(42, "Foo");
    s.pkg_id = 1;
    b.symbols.push(s);
    b.defs.push(DefRecord {
        sym_id: 42,
        file_id: 7,
        start_line: 10,
        start_col: 0,
        end_line: 20,
        end_col: 1,
        // Enclosing-item body extent spanning beyond the name span — must
        // round-trip through both DefRow and DefLineRow.
        body_start_line: 8,
        body_end_line: 25,
    });
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 42,
        sig: "fn foo()".into(),
        doc: "the foo".into(),
    });
    b.edges.push(EdgeRecord {
        src_id: 99,
        target_id: 7,
        properties: EdgeProperties::Contains,
    });
    w.write_batch(&b).unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    assert_eq!(
        r.fetch_symbol("rust", "rs:Foo").await.unwrap().unwrap().id,
        42
    );
    assert_eq!(
        r.fetch_symbol_pub_id(42).await.unwrap().as_deref(),
        Some("rs:Foo")
    );
    let defs = r.fetch_defs(42).await.unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].start_line, 10);
    assert_eq!((defs[0].body_start_line, defs[0].body_end_line), (8, 25));
    let dl = r.fetch_def_lines(42).await.unwrap();
    assert_eq!(dl[0].end_line, 20);
    assert_eq!((dl[0].body_start_line, dl[0].body_end_line), (8, 25));
    let at = r.find_at_location(7, 15).await.unwrap();
    assert_eq!(at.len(), 1);
    assert_eq!(at[0].id, 42);
    assert!(r.find_at_location(7, 99).await.unwrap().is_empty());
    assert_eq!(r.fetch_package(1).await.unwrap().unwrap().name, "kenn");
    assert_eq!(
        r.distinct_packages().await.unwrap(),
        vec!["kenn".to_string()]
    );
    assert_eq!(
        r.fetch_symbol_docs_row(42).await.unwrap().unwrap().sig,
        "fn foo()"
    );
    let (mf, total) = r.list_module_files(99, 10, None).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(mf[0].path, "src/a.rs");
}

#[tokio::test]
async fn blended_search_fuses_name_and_doc() {
    use kenn_model::SymbolDocsRecord;
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "parseUser"));
    b.symbols.push(sym(2, "helper"));
    // `helper`'s doc mentions "parse" — should surface via the doc arm.
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 2,
        sig: "fn helper()".into(),
        doc: "parse the input".into(),
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    let hits = r
        .search_symbols_blended("parse", None, 10, false, false)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|h| h.symbol.name.as_str()).collect();
    // name arm finds parseUser; doc arm finds helper.
    assert!(names.contains(&"parseUser"), "got {names:?}");
    assert!(names.contains(&"helper"), "got {names:?}");
    // parseUser hits the name_lower (prefix) + signature arms; helper only the
    // doc arm — so under RRF the multi-arm hit outranks the doc-only one.
    assert_eq!(hits[0].symbol.name, "parseUser");
}

#[tokio::test]
async fn tiered_word_split_finds_snake_and_camel_case() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "cancel_order"));
    b.symbols.push(sym(2, "CancelOrder"));
    b.symbols.push(sym(3, "unrelated_thing"));
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    // Query by the words, separator-agnostic: both casings are found, the
    // unrelated symbol is not.
    let hits = r
        .find_symbol_tiered("cancel order", 10, false, false)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|h| h.symbol.name.as_str()).collect();
    assert!(names.contains(&"cancel_order"), "got {names:?}");
    assert!(names.contains(&"CancelOrder"), "got {names:?}");
    assert!(!names.contains(&"unrelated_thing"), "got {names:?}");
}

#[tokio::test]
async fn fts5_match_tolerates_punctuation_and_operator_words() {
    use kenn_model::SymbolDocsRecord;
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "cancel_order"));
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: "fn cancel_order()".into(),
        doc: "cancel the pending order".into(),
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    // Hyphens, the FTS5 operator word `OR`/`NEAR`, and a stray quote must all
    // normalize to a valid MATCH (no syntax error) on every arm.
    for q in [
        "cancel-order",
        "cancel OR order",
        "NEAR cancel",
        "cancel\"order",
    ] {
        r.search_symbols_blended(q, None, 10, false, false)
            .await
            .unwrap_or_else(|e| panic!("blended({q:?}) errored: {e:?}"));
        r.find_symbol_tiered(q, 10, false, false)
            .await
            .unwrap_or_else(|e| panic!("tiered({q:?}) errored: {e:?}"));
    }
}

#[tokio::test]
async fn scans_return_symbols_and_edges() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "a"));
    b.symbols.push(sym(2, "b"));
    b.edges.push(EdgeRecord {
        src_id: 1,
        target_id: 2,
        properties: EdgeProperties::Calls,
    });
    w.write_batch(&b).unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    assert_eq!(r.scan_symbols().await.unwrap().len(), 2);
    let calls = r.scan_edges(EdgeKind::Calls.db_name()).await.unwrap();
    assert_eq!(calls, vec![(1, 2)]);
    r.scan_edges("bogus").await.unwrap_err();
}

#[tokio::test]
async fn find_similar_returns_nearest_other_symbol() {
    use crate::db::codes::text_fingerprint;
    use crate::embed::sidecar::io::{append_vectors, WriterPrefix};
    use crate::embed::sidecar::manifest::{Manifest, CODE_TEXT_RECIPE};
    use kenn_model::SymbolDocsRecord;

    // Committed vectors for two documented symbols, keyed by the `doc` recipe
    // fingerprint (over the doc prose) that `finalize` computes.
    let doc_alpha = "alpha handles the first concern";
    let doc_beta = "beta handles the second concern";
    let fp = text_fingerprint;
    let mut v_alpha = vec![0.0_f32; 768];
    v_alpha[0] = 1.0;
    let mut v_beta = vec![0.0_f32; 768];
    v_beta[1] = 1.0;

    let dir = TempDir::new().unwrap();
    let sidecar = dir.path().join("vectors");
    let tmp = sidecar.join(".tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    append_vectors(
        &sidecar,
        &tmp,
        WriterPrefix::Seg,
        768,
        &[(fp(doc_alpha), v_alpha), (fp(doc_beta), v_beta)],
    )
    .unwrap();
    Manifest::current("test-model".to_owned(), 768, CODE_TEXT_RECIPE)
        .write(&sidecar)
        .unwrap();

    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions {
            vectors_dir: Some(sidecar.clone()),
            vectors_model_id: Some("test-model".to_owned()),
            ..crate::api::types::WriterOptions::default()
        },
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "alpha"));
    b.symbols.push(sym(2, "beta"));
    b.symbols.push(sym(3, "gamma")); // no doc → no committed vector
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 1,
        sig: String::new(),
        doc: doc_alpha.into(),
    });
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 2,
        sig: String::new(),
        doc: doc_beta.into(),
    });
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    let hits = r
        .find_similar_symbols(1, 10, false, false)
        .await
        .unwrap()
        .expect("symbol 1 has a committed vector");
    let ids: Vec<u32> = hits.iter().map(|h| h.id).collect();
    assert!(!ids.contains(&1), "the source symbol is excluded: {ids:?}");
    assert!(
        ids.contains(&2),
        "the other embedded symbol is returned: {ids:?}"
    );
    // The gap fix: a symbol with no committed vector returns None (the
    // "vectors not built" signal), distinct from Some(empty) ("no neighbours").
    assert!(
        r.find_similar_symbols(3, 10, false, false)
            .await
            .unwrap()
            .is_none(),
        "a symbol with no committed vector signals None, not an empty list"
    );
}

#[tokio::test]
async fn implements_the_reader_trait() {
    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    b.symbols.push(sym(1, "parseUser"));
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();
    // The async Reader trait surface, dispatched onto the pool.
    assert_eq!(Reader::count_table(&r, "symbols").await.unwrap(), 1);
    let s = Reader::fetch_symbol_by_short_id(&r, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.name, "parseUser");
    let (hits, total) = Reader::search_symbols_by_name(&r, "parse", 10, None, None, false, false)
        .await
        .unwrap();
    assert!(!hits.is_empty());
    assert_eq!(total, hits.len() as u64);
    let tiered = Reader::find_symbol_tiered(&r, "parseUser", 10, false, false)
        .await
        .unwrap();
    assert_eq!(tiered[0].match_kind.as_str(), "exact");
    assert!(Reader::scan_aggregate_nodes(&r).await.unwrap().is_empty());
}

#[tokio::test]
async fn code_lookups_match_basename_and_short_name_excluding_markdown_and_external() {
    use kenn_model::DefRecord;

    let dir = TempDir::new().unwrap();
    let w = super::super::writer::SqliteWriter::create(
        dir.path(),
        crate::api::types::WriterOptions::default(),
    )
    .unwrap();
    let mut b = WriteBatch::default();
    // Two code files sharing a basename, one external, plus a markdown file.
    let mkfile = |id: u32, path: &str, lang: Language, external: bool| FileRecord {
        id,
        path: path.into(),
        language: lang,
        test: false,
        external,
        content_hash: 1,
    };
    b.files
        .push(mkfile(1, "api/order.rs", Language::Rust, false));
    b.files
        .push(mkfile(2, "ui/order.rs", Language::Rust, false));
    b.files
        .push(mkfile(3, "vendor/order.rs", Language::Rust, true));
    b.files
        .push(mkfile(4, "docs/order.md", Language::Markdown, false));
    // A code symbol + a markdown section symbol that share a short name.
    let mut code_sym = sym(10, "OrderHandler");
    code_sym.pub_id = "rs:billing::OrderHandler".into();
    b.symbols.push(code_sym);
    b.symbols.push(SymbolRecord {
        id: 11,
        pub_id: "md:workspace/docs/order.md#orderhandler".into(),
        language: Language::Markdown,
        pkg_id: 0,
        kind: Kind::Section,
        name: "OrderHandler".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    let def = |sym_id: u32, file_id: u32| DefRecord {
        sym_id,
        file_id,
        start_line: 1,
        start_col: 0,
        end_line: 9,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    };
    b.defs.push(def(10, 1)); // code symbol lives in api/order.rs
    b.defs.push(def(11, 4)); // md section lives in the markdown file
    w.write_batch(&b).unwrap();
    w.finalize().unwrap();
    drop(w);

    let r = SqliteReader::open(dir.path()).await.unwrap();

    // basename: the two internal code files, never the external or markdown one.
    let mut files = r.files_by_basename("order.rs").await.unwrap();
    files.sort_by_key(|f| f.id);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["api/order.rs", "ui/order.rs"]);

    // short name: the code symbol only — the markdown section is excluded.
    let syms = r.symbols_by_short_name("orderhandler").await.unwrap();
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0].id, 10);
    assert_eq!(syms[0].qualified, "rs:billing::OrderHandler");
    assert_eq!(syms[0].relpath, "api/order.rs");
}
