//! Search-ranking parity gate (replace-lance-with-sqlite, task 4.4).
//!
//! The original plan diffed `SQLite`'s top-k against `fixtures/lance_baseline.json`
//! (frozen in task 0.1). That gate is not runnable: the baseline's corpus is
//! kenn's *own* source, and this change refactored kenn-store between the freeze
//! and the swap, so the baseline's `kenn-store::db::*` `pub_ids` no longer exist —
//! a re-indexed tree would mismatch from corpus drift, not a ranking regression,
//! and Lance is deleted so no fresh baseline can be captured. The fixture stays
//! in the tree as a captured historical reference (see the change's design note),
//! and parity is asserted here as the ranking *policy* §4.1–4.3 promise, on a
//! fixed in-test corpus that never drifts.
//!
//! The vector arm is not exercised here: per design D5 it is exact (vs Lance's
//! approximate `IVF_PQ`) and validated by NN sanity, not overlap. Real-model
//! hybrid retrieval lives in `hybrid_search.rs`.

use kenn_store::api::{Reader, WriteBatch, WriterOptions};
use kenn_store::{open_reader, open_writer};

use kenn_model::{FileRecord, Kind, Language, SymbolDocsRecord, SymbolRecord};

fn func(id: u32, name: &str) -> SymbolRecord {
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

/// A fixed corpus exercising the ranking policy:
/// - `parse` — the whole-name exact-match target,
/// - `parseUser` / `reparser` — substring (trigram) matches, no exact boost,
/// - `helper` — name does NOT contain the query; its *doc* does (doc-only).
async fn corpus_reader(dir: &std::path::Path) -> impl Reader {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: 1,
        path: "src/lib.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(func(1, "parse"));
    b.symbols.push(func(2, "parseUser"));
    b.symbols.push(func(3, "reparser"));
    b.symbols.push(func(4, "helper"));
    b.symbol_docs.push(SymbolDocsRecord {
        sym_id: 4,
        sig: "fn helper()".into(),
        doc: "parse the input stream".into(),
    });
    writer.write_batch(&b).await.expect("write_batch");
    writer.finalize().await.expect("finalize");
    drop(writer);

    open_reader(dir).await.expect("open_reader")
}

#[tokio::test(flavor = "multi_thread")]
async fn identifier_search_boosts_exact_and_excludes_doc_only() {
    let dir = tempfile::tempdir().unwrap();
    let reader = corpus_reader(dir.path()).await;

    let (hits, _) = Reader::search_symbols_by_name(&reader, "parse", 10, None, None, false, false)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();

    // §4.1: the whole-name exact match is boosted to rank 0 over substring hits.
    assert_eq!(
        names.first(),
        Some(&"parse"),
        "exact match first: {names:?}"
    );
    // Trigram retrieval still surfaces the substring matches.
    assert!(names.contains(&"parseUser"), "{names:?}");
    assert!(names.contains(&"reparser"), "{names:?}");
    // Identifier search is name-only: the doc-only match never appears here.
    assert!(
        !names.contains(&"helper"),
        "doc-only match must not leak into identifier search: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn blended_surfaces_doc_only_below_name_match() {
    let dir = tempfile::tempdir().unwrap();
    let reader = corpus_reader(dir.path()).await;

    let blended = Reader::search_symbols_blended(&reader, "parse", None, 10, false, false)
        .await
        .unwrap();
    let names: Vec<&str> = blended.iter().map(|h| h.symbol.name.as_str()).collect();

    // §4.2: the doc arm surfaces the doc-only match that identifier search omits.
    assert!(
        names.contains(&"helper"),
        "doc arm missing helper: {names:?}"
    );
    // The name arm is weighted 3× the doc arm, so a name match outranks a
    // doc-only match.
    let pos = |n: &str| names.iter().position(|x| *x == n).expect("present");
    assert!(
        pos("parse") < pos("helper"),
        "name match should outrank doc-only: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn identifier_search_is_deterministic_and_floors_short_queries() {
    let dir = tempfile::tempdir().unwrap();
    let reader = corpus_reader(dir.path()).await;

    // The trigram tokenizer needs ≥3 alphanumeric chars; shorter queries
    // return nothing rather than erroring.
    let (short, _) = Reader::search_symbols_by_name(&reader, "ab", 10, None, None, false, false)
        .await
        .unwrap();
    assert!(
        short.is_empty(),
        "sub-trigram query must be empty: {short:?}"
    );

    // The `(score DESC, len(name) ASC, id ASC)` total order is deterministic
    // across identical queries.
    let ids = |hits: &[kenn_store::api::types::RankedSymbolRow]| {
        hits.iter().map(|h| h.id).collect::<Vec<_>>()
    };
    let (a, _) = Reader::search_symbols_by_name(&reader, "parse", 10, None, None, false, false)
        .await
        .unwrap();
    let (b, _) = Reader::search_symbols_by_name(&reader, "parse", 10, None, None, false, false)
        .await
        .unwrap();
    assert_eq!(ids(&a), ids(&b), "ranking must be deterministic");
}
