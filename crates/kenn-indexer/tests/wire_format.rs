//! Task 2.10 — snapshot the JSON wire format of every record type the
//! producer emits. `kenn-model` owns the types; this test locks the
//! producer's serialization.

use kenn_indexer::{RunReport, RunStatus};
use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FieldOp, FileRecord, ImportKind, IsomorphismSource,
    Kind, Language, SymbolDocsRecord, SymbolRecord,
};

fn dump<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("serialize value")
}

#[test]
fn file_record_wire_format() {
    let r = FileRecord {
        id: 1,
        path: "src/lib.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 0xdead_beef,
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn symbol_record_wire_format() {
    let r = SymbolRecord {
        id: 42,
        pub_id: "rs:foo::bar".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "bar".into(),
        enclosing_sym_id: 1,
        partial: false,
        nargs: 2,
        targs: 1,
        external: false,
        test: false,
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn symbol_docs_record_wire_format() {
    let r = SymbolDocsRecord {
        sym_id: 42,
        sig: "fn bar<T>(x: T) -> T".into(),
        doc: "/// Returns its argument.".into(),
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn def_record_wire_format() {
    let r = DefRecord {
        sym_id: 7,
        file_id: 2,
        start_line: 10,
        start_col: 0,
        end_line: 80,
        end_col: 1,
        body_start_line: 0,
        body_end_line: 0,
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn edge_record_wire_format_calls() {
    let r = EdgeRecord {
        src_id: 2,
        target_id: 5,
        properties: EdgeProperties::Calls,
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn edge_record_wire_format_field_access() {
    let r = EdgeRecord {
        src_id: 2,
        target_id: 8,
        properties: EdgeProperties::FieldAccess { op: FieldOp::Write },
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn edge_record_wire_format_imports() {
    let r = EdgeRecord {
        src_id: 1,
        target_id: 9,
        properties: EdgeProperties::Imports {
            kind: ImportKind::ReExport,
        },
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn edge_record_wire_format_corresponds_to() {
    let r = EdgeRecord {
        src_id: 12,
        target_id: 13,
        properties: EdgeProperties::CorrespondsTo {
            source: IsomorphismSource::Config,
            generator: String::new(),
            canonical: 12,
        },
    };
    insta::assert_snapshot!(dump(&r));
}

#[test]
fn run_report_wire_format() {
    let mut r = RunReport::started_for(
        kenn_model::Language::Csharp,
        "kenn-dotnet",
        "0.1.0",
        "fixture.sln",
    );
    r.started_at = "2026-05-02T18:00:00Z".into();
    r.ended_at = "2026-05-02T18:00:42Z".into();
    r.files_seen = 17;
    r.symbols_seen = 1234;
    r.defs_seen = 2345;
    r.edges_seen = 3210;
    r.failed_projects = vec!["BrokenProj".into()];
    r.failed_overflow = 3;
    r.status = RunStatus::Partial;
    insta::assert_snapshot!(dump(&r));
}
