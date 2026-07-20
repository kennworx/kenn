//! Cross-language ID tests (tasks 1.8, 1.9, 1.10).
//!
//! C# is intentionally absent: it goes through the JSONL ingest path
//! (kenn-dotnet emits `key`-bearing `SymbolFrame`s consumed directly by
//! `transform_jsonl`) and never through the SCIP transformer chain.

use kenn_model::id::{
    GoTransformer, IdTransformer, PythonTransformer, RustTransformer, TypeScriptTransformer,
};
use std::collections::HashSet;

#[test]
fn round_trip_idempotent_across_fixture_corpus() {
    let cases: Vec<(Box<dyn IdTransformer>, &str)> = vec![
        (
            Box::new(TypeScriptTransformer),
            "scip-typescript npm @acme/frontend-shared 1.0.0 src/api/`AppError`#",
        ),
        (
            Box::new(RustTransformer),
            "rust-analyzer cargo quinn_proto 0.10.0 connection/Connection#new().",
        ),
        (
            Box::new(RustTransformer),
            "rust-analyzer cargo foo 0.1.0 impl#[MyType][MyTrait]some_method().",
        ),
        (
            Box::new(GoTransformer),
            "scip-go gomod github.com/foo/quinn-proto 0.1.0 \
             `github.com/foo/quinn-proto/connection`/Connection#New().",
        ),
        (
            Box::new(PythonTransformer),
            "scip-python python click 8.1.0 click/core/Context#invoke().",
        ),
    ];
    for (t, scip) in cases {
        let a = t.scip_to_public(scip).expect("scip_to_public");
        let b = t.scip_to_public(scip).expect("scip_to_public second pass");
        assert_eq!(a, b, "non-idempotent for {scip}");
        let parsed = t.parse_public(a.as_str()).expect("parse_public");
        assert_eq!(parsed.language, t.language());
    }
}

#[test]
fn no_collisions_within_language() {
    let go = GoTransformer;
    let inputs = [
        "scip-go gomod A 0.1.0 N/T#Foo().",
        "scip-go gomod A 0.1.0 N/T#Bar().",
        "scip-go gomod A 0.1.0 N/T#Baz().",
        "scip-go gomod A 0.1.0 N/Other#Foo().",
    ];
    let mut seen: HashSet<String> = HashSet::new();
    for s in inputs {
        let id = go.scip_to_public(s).unwrap();
        assert!(seen.insert(id.into_string()), "collision on {s}");
    }
    assert_eq!(seen.len(), inputs.len());
}

#[test]
fn cross_language_prefix_disambiguates() {
    let rs = RustTransformer
        .scip_to_public("rust-analyzer cargo A 1.0.0 foo::Bar.")
        .unwrap();
    let go = GoTransformer
        .scip_to_public("scip-go gomod A 0.1.0 `A`/Foo#Bar().")
        .unwrap();
    assert_ne!(rs, go);
    assert!(rs.as_str().starts_with("rs:"));
    assert!(go.as_str().starts_with("go:"));
}

#[test]
fn id_stable_across_pkg_version_change() {
    // Task 1.10: a fixture pair where only the package version changes —
    // public ID is identical because version is metadata, not part of the ID.
    let v1 = GoTransformer
        .scip_to_public("scip-go gomod A 0.1.0 N/T#Foo().")
        .unwrap();
    let v2 = GoTransformer
        .scip_to_public("scip-go gomod A 2.5.0 N/T#Foo().")
        .unwrap();
    assert_eq!(v1, v2);
}
