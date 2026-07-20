use super::*;

use kenn_model::Language;
use scip::types::Document;

use crate::transform::{IdRegistry, TransformError};

#[test]
fn file_docs_coalesce_header_and_module_docs() {
    let src = "// Copyright foo\n\n//! Module purpose line one\n//! line two\n\nuse std::io;\nfn main() {}";
    let blocks = extract_rust_file_docs(src);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0], "// Copyright foo");
    assert_eq!(blocks[1], "//! Module purpose line one\n//! line two");
}

#[test]
fn file_docs_block_comment_is_one_entry() {
    let src = "/* header\n   line two */\nuse std::io;";
    let blocks = extract_rust_file_docs(src);
    assert_eq!(blocks, vec!["/* header\n   line two */".to_string()]);
}

#[test]
fn file_docs_skip_inner_attributes_keep_scanning() {
    let src = "#![allow(dead_code)]\n//! module docs\nfn x() {}";
    let blocks = extract_rust_file_docs(src);
    assert_eq!(blocks, vec!["//! module docs".to_string()]);
}

#[test]
fn file_docs_stop_at_first_code() {
    let src = "//! docs\nfn x() {}\n// trailing comment not captured";
    let blocks = extract_rust_file_docs(src);
    assert_eq!(blocks, vec!["//! docs".to_string()]);
}

#[test]
fn file_docs_empty_when_no_leading_comments() {
    assert!(extract_rust_file_docs("use std::io;\nfn main() {}").is_empty());
}

/// Build a tiny SCIP `Document` with one symbol so
/// `transform_document` has something real to walk.
fn synthetic_document(language: &str, sym: &str, doc_text: &str) -> Document {
    use scip::types::{Document, SymbolInformation};
    let mut info = SymbolInformation::new();
    info.symbol = sym.into();
    if !doc_text.is_empty() {
        info.documentation = vec![doc_text.into()];
    }
    let mut doc = Document::new();
    doc.language = language.into();
    doc.relative_path = "src/lib.rs".into();
    doc.symbols = vec![info];
    doc
}

fn synthetic_workspace() -> (tempfile::TempDir, crate::Workspace) {
    let dir = tempfile::TempDir::new().unwrap();
    // Materialize the file so canonicalize() succeeds for occurrence
    // paths the document references.
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "// fixture\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[]).expect("workspace");
    (dir, ws)
}

/// `transform_document` for a recognized SCIP language with a
/// well-formed symbol emits a `FileRecord` (new file) and a
/// `SymbolRecord`. The `SymbolDocsRecord` is also emitted when
/// documentation is non-empty.
#[test]
fn transform_document_happy_path_for_rust() {
    let (dir, ws) = synthetic_workspace();
    let doc = synthetic_document("rust", "rust-analyzer cargo k 0.1 m/Foo#", "the foo type");
    let mut reg = IdRegistry::new(Language::Rust);
    // Canonicalize so the URI matches whatever path the workspace
    // resolves to (macOS /var/ ↔ /private/var/ symlink quirk).
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_some(), "first encounter emits a FileRecord");
    assert_eq!(out.symbols.len(), 1, "one symbol consumed");
    assert!(!out.docs.is_empty(), "documentation produces a docs row");
}

/// Empty SCIP `Document.language` falls back to extension
/// inference via `language_from_path`. With a `.rs` extension,
/// the function still produces a record.
#[test]
fn transform_document_falls_back_to_path_extension() {
    let (dir, ws) = synthetic_workspace();
    let mut doc = synthetic_document("", "rust-analyzer cargo k 0.1 m/Foo#", "");
    doc.relative_path = "src/lib.rs".into();
    let mut reg = IdRegistry::new(Language::Rust);
    // Canonicalize so the URI matches whatever path the workspace
    // resolves to (macOS /var/ ↔ /private/var/ symlink quirk).
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let _ = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
}

/// Python document whose `relative_path` matches an exclude pattern
/// is dropped — `TransformedDocument` has no file, no symbols.
#[test]
fn transform_document_drops_python_doc_matching_exclude() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("worked/httpx/raw")).unwrap();
    std::fs::write(
        dir.path().join("worked/httpx/raw/transport.py"),
        "# fixture\n",
    )
    .unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &["worked/**".to_string()])
        .unwrap();
    let mut doc = synthetic_document("python", "scip-python python pkg 0.1 worked/httpx/raw/", "");
    doc.relative_path = "worked/httpx/raw/transport.py".into();
    let mut reg = IdRegistry::new(Language::Python);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_none(), "excluded doc emits no FileRecord");
    assert!(out.symbols.is_empty(), "excluded doc emits no symbols");
    assert!(out.defs.is_empty());
    assert!(out.edges.is_empty());
}

/// Same Python doc with empty `exclude_documents` → ingested normally
/// (`FileRecord` + `SymbolRecord` land).
#[test]
fn transform_document_keeps_python_doc_when_no_exclude_configured() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("worked/httpx/raw")).unwrap();
    std::fs::write(
        dir.path().join("worked/httpx/raw/transport.py"),
        "# fixture\n",
    )
    .unwrap();
    let ws = crate::Workspace::new(dir.path(), &[]).unwrap();
    let mut doc = synthetic_document(
        "python",
        "scip-python python pkg 0.1 worked/httpx/raw/transport/Foo#",
        "",
    );
    doc.relative_path = "worked/httpx/raw/transport.py".into();
    let mut reg = IdRegistry::new(Language::Python);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_some(), "with no exclude, doc is ingested");
}

/// Non-matching Python doc passes through even with patterns
/// configured.
#[test]
fn transform_document_keeps_python_doc_when_pattern_misses() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("graphify")).unwrap();
    std::fs::write(dir.path().join("graphify/detect.py"), "x = 1\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &["worked/**".to_string()])
        .unwrap();
    let mut doc = synthetic_document(
        "python",
        "scip-python python pkg 0.1 graphify/detect/Foo#",
        "",
    );
    doc.relative_path = "graphify/detect.py".into();
    let mut reg = IdRegistry::new(Language::Python);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_some(), "non-matching doc still ingests");
}

/// OR-semantics: a doc dropped by the SECOND pattern is still dropped.
#[test]
fn transform_document_or_semantics_across_multiple_patterns() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("tests/fixtures")).unwrap();
    std::fs::write(dir.path().join("tests/fixtures/sample.py"), "# sample\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(
            Language::Python,
            &["worked/**".to_string(), "tests/fixtures/**".to_string()],
        )
        .unwrap();
    let mut doc = synthetic_document(
        "python",
        "scip-python python pkg 0.1 tests/fixtures/sample/Foo#",
        "",
    );
    doc.relative_path = "tests/fixtures/sample.py".into();
    let mut reg = IdRegistry::new(Language::Python);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_none(), "second pattern still drops");
}

/// Test file under `tests/` but NOT matching `exclude_documents` still
/// ingests (composes with patterns).
#[test]
fn transform_document_keeps_tests_dir_when_pattern_misses() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(dir.path().join("tests/test_detect.py"), "x = 1\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(
            Language::Python,
            &["worked/**".to_string(), "tests/fixtures/**".to_string()],
        )
        .unwrap();
    let mut doc = synthetic_document(
        "python",
        "scip-python python pkg 0.1 tests/test_detect/Foo#",
        "",
    );
    doc.relative_path = "tests/test_detect.py".into();
    let mut reg = IdRegistry::new(Language::Python);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_some(), "tests/test_*.py outside fixtures stays");
}

/// The per-language exclude filter generalizes: a Rust document
/// whose path matches `[language.rust].excludes` is dropped, just
/// like a Python doc would be. Proves the `is_excluded(language, ...)`
/// machinery works for every language, not just Python.
#[test]
fn transform_document_drops_rust_doc_matching_rust_exclude() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    std::fs::write(dir.path().join("target/debug/build.rs"), "// fixture\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Rust, &["target/**".to_string()])
        .unwrap();
    let mut doc = synthetic_document("rust", "rust-analyzer cargo k 0.1 m/Foo#", "");
    doc.relative_path = "target/debug/build.rs".into();
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.file.is_none(), "rust exclude must drop the doc");
    assert!(out.symbols.is_empty());
}

/// Cross-language scoping at the transform layer: a Rust doc is
/// unaffected by Python's excludes (regression guard for the
/// kenn-per-language-excludes split).
#[test]
fn transform_document_does_not_filter_rust_doc_with_python_exclude() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("__pycache__")).unwrap();
    std::fs::write(dir.path().join("__pycache__/foo.rs"), "// fixture\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &["__pycache__/**".to_string()])
        .unwrap();
    // No Rust excludes attached.
    let mut doc = synthetic_document("rust", "rust-analyzer cargo k 0.1 m/Foo#", "");
    doc.relative_path = "__pycache__/foo.rs".into();
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(
        out.file.is_some(),
        "Rust doc must not be filtered by Python's exclude set",
    );
}

/// Non-Python documents are unaffected by `python_excludes` even
/// when their `relative_path` would match the pattern.
#[test]
fn transform_document_does_not_filter_non_python_docs() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("worked/x")).unwrap();
    std::fs::write(dir.path().join("worked/x/lib.rs"), "// fixture\n").unwrap();
    let ws = crate::Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &["worked/**".to_string()])
        .unwrap();
    let mut doc = synthetic_document("rust", "rust-analyzer cargo k 0.1 m/Foo#", "");
    doc.relative_path = "worked/x/lib.rs".into();
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(
        out.file.is_some(),
        "python_excludes must NOT filter non-python docs",
    );
}

/// Unrecognized SCIP language AND no path-based inference →
/// `TransformError::UnknownLanguage`.
#[test]
fn transform_document_unknown_language_errors() {
    let (dir, ws) = synthetic_workspace();
    let mut doc = synthetic_document("klingon", "klingon foo", "");
    doc.relative_path = "src/lib.klingon".into();
    let mut reg = IdRegistry::new(Language::Rust);
    // Canonicalize so the URI matches whatever path the workspace
    // resolves to (macOS /var/ ↔ /private/var/ symlink quirk).
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    match transform_document(&doc, &ws, &uri, &mut reg) {
        Err(TransformError::UnknownLanguage(_)) => {}
        Ok(_) => panic!("unknown language must error"),
        Err(e) => panic!("expected UnknownLanguage, got {e:?}"),
    }
}

/// Definition occurrences in the SCIP document yield 1-based `DefRecord`
/// lines (per `source-data-model` D1). A symbol with a Definition
/// occurrence at 0-based range `[9, 4, 9, 24]` MUST land in the store
/// as `start_line = 10, end_line = 10` (columns unchanged).
#[test]
fn transform_document_populates_def_range_from_definition_occurrence() {
    use scip::types::{Occurrence, SymbolRole};
    let (dir, ws) = synthetic_workspace();
    let scip_sym = "rust-analyzer cargo k 0.1 m/Foo#";
    let mut doc = synthetic_document("rust", scip_sym, "");
    let mut occ = Occurrence::new();
    occ.symbol = scip_sym.into();
    occ.range = vec![9, 4, 9, 24];
    occ.symbol_roles = SymbolRole::Definition as i32;
    doc.occurrences = vec![occ];
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert_eq!(out.defs.len(), 1, "one DefRecord per definition occurrence");
    let d = &out.defs[0];
    assert_eq!(d.start_line, 10, "0-based 9 → 1-based 10");
    assert_eq!(d.start_col, 4, "columns pass through unchanged");
    assert_eq!(d.end_line, 10);
    assert_eq!(d.end_col, 24);
    assert_eq!(
        (d.body_start_line, d.body_end_line),
        (0, 0),
        "no enclosing_range → absent body extent"
    );
}

/// A definition occurrence carrying SCIP `enclosing_range` (the whole item
/// body — rust-analyzer ≥ Dec-2025 / scip-go / scip-python) populates the
/// `DefRecord` body extent, 1-based and distinct from the name span. The
/// body starts above the name (the doc comment).
#[test]
fn transform_document_populates_body_extent_from_enclosing_range() {
    use scip::types::{Occurrence, SymbolRole};
    let (dir, ws) = synthetic_workspace();
    let scip_sym = "rust-analyzer cargo k 0.1 m/foo().";
    let mut doc = synthetic_document("rust", scip_sym, "");
    let mut occ = Occurrence::new();
    occ.symbol = scip_sym.into();
    occ.range = vec![9, 4, 9, 7]; // name span: `foo` on 0-based line 9
    occ.enclosing_range = vec![8, 0, 20, 1]; // body: 0-based line 8 → 20
    occ.symbol_roles = SymbolRole::Definition as i32;
    doc.occurrences = vec![occ];
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert_eq!(out.defs.len(), 1);
    let d = &out.defs[0];
    assert_eq!((d.start_line, d.end_line), (10, 10), "name span 1-based");
    assert_eq!(
        (d.body_start_line, d.body_end_line),
        (9, 21),
        "enclosing 0-based [8,20] → 1-based [9,21]"
    );
    assert!(
        d.body_start_line < d.start_line,
        "doc comment sits above the name line"
    );
}

/// A 3-int single-line `enclosing_range` (`[line, start_col, end_col]`) maps
/// to a single-line body span.
#[test]
fn transform_document_body_extent_from_single_line_enclosing_range() {
    use scip::types::{Occurrence, SymbolRole};
    let (dir, ws) = synthetic_workspace();
    let scip_sym = "rust-analyzer cargo k 0.1 m/C#";
    let mut doc = synthetic_document("rust", scip_sym, "");
    let mut occ = Occurrence::new();
    occ.symbol = scip_sym.into();
    occ.range = vec![4, 7, 4, 8]; // name
    occ.enclosing_range = vec![4, 0, 30]; // 3-int single-line form
    occ.symbol_roles = SymbolRole::Definition as i32;
    doc.occurrences = vec![occ];
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    let d = &out.defs[0];
    assert_eq!(
        (d.body_start_line, d.body_end_line),
        (5, 5),
        "0-based 4 → 5"
    );
}

/// A SCIP symbol with no Definition occurrence in this document still
/// gets a placeholder `DefRecord` so the symbol stays addressable by
/// `file_id`; the line/col values are zero. Externality is set by the
/// stub-flush path, not here.
#[test]
fn transform_document_emits_placeholder_def_when_no_definition_occurrence() {
    let (dir, ws) = synthetic_workspace();
    let doc = synthetic_document("rust", "rust-analyzer cargo k 0.1 m/Foo#", "");
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert_eq!(out.defs.len(), 1);
    let d = &out.defs[0];
    assert_eq!(
        (d.start_line, d.start_col, d.end_line, d.end_col),
        (0, 0, 0, 0)
    );
}

/// Two Definition occurrences for the same symbol (cfg-gated partials,
/// per `scip-indexer` D2.4) produce two `DefRecord` rows sharing
/// `sym_id` with distinct line positions.
#[test]
fn transform_document_emits_one_def_per_definition_occurrence() {
    use scip::types::{Occurrence, SymbolRole};
    let (dir, ws) = synthetic_workspace();
    let scip_sym = "rust-analyzer cargo k 0.1 m/Foo#bar().";
    let mut doc = synthetic_document("rust", scip_sym, "");
    let occ = |sl: i32| {
        let mut o = Occurrence::new();
        o.symbol = scip_sym.into();
        o.range = vec![sl, 0, sl, 8];
        o.symbol_roles = SymbolRole::Definition as i32;
        o
    };
    doc.occurrences = vec![occ(3), occ(17)];
    let mut reg = IdRegistry::new(Language::Rust);
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert_eq!(out.defs.len(), 2, "one DefRecord per Definition occurrence");
    let lines: Vec<u32> = out.defs.iter().map(|d| d.start_line).collect();
    assert_eq!(lines, vec![4, 18], "0-based 3,17 → 1-based 4,18");
    assert_eq!(out.defs[0].sym_id, out.defs[1].sym_id, "rows share sym_id");
}

/// SCIP `local NNN` symbols are intra-document; the transformer
/// silently drops them rather than erroring on the unparseable id.
#[test]
fn transform_document_skips_local_scip_symbols() {
    let (dir, ws) = synthetic_workspace();
    let doc = synthetic_document("rust", "local 1", "");
    let mut reg = IdRegistry::new(Language::Rust);
    // Canonicalize so the URI matches whatever path the workspace
    // resolves to (macOS /var/ ↔ /private/var/ symlink quirk).
    let uri = format!("file://{}", dir.path().canonicalize().unwrap().display());
    let out = transform_document(&doc, &ws, &uri, &mut reg).expect("transform_document");
    assert!(out.symbols.is_empty(), "local symbol must be filtered");
}
