//! Symbol classification and naming derived from SCIP symbol strings:
//! `Kind` derivation, cross-document stub interning, display/short-name
//! extraction, parent resolution, and the per-language test-descriptor
//! heuristic.

use kenn_model::{Kind, Language, ShortId, SymbolRecord};
use scip::types::SymbolInformation;

use super::IdRegistry;

/// Tier of the symbol-kind classifier (5c.5) — recorded in the run report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindSource {
    Scip,
    Descriptor,
    Unknown,
}

pub fn derive_kind_with_source(info: &SymbolInformation) -> (Kind, KindSource) {
    use kenn_model::kind_classifier::{
        kind_from_descriptor_suffix, kind_from_scip_go_kind, ScipKind,
    };
    let raw = info.kind.value();
    if raw != 0 {
        if let Some(sk) = ScipKind::from_i32(raw) {
            return (kind_from_scip_go_kind(sk), KindSource::Scip);
        }
    }
    if let Some(k) = kind_from_descriptor_suffix(strip_scip_head(&info.symbol)) {
        return (k, KindSource::Descriptor);
    }
    (Kind::Variable, KindSource::Unknown)
}

pub(crate) fn derive_kind(info: &SymbolInformation) -> Kind {
    derive_kind_with_source(info).0
}

/// Strip the `<scheme> <manager> <package> <version>` head from a SCIP
/// symbol, leaving only the descriptor — input to the descriptor-grammar
/// classifier. Returns the empty slice for malformed symbols.
fn strip_scip_head(scip: &str) -> &str {
    let mut parts = scip.splitn(5, ' ');
    parts.nth(4).unwrap_or("")
}

/// Byte length of the `<scheme> <manager> <package> <version> ` head
/// (including the trailing space). Returns `None` for malformed
/// (`<head> <descriptor>`-shaped) symbols missing the four spaces.
fn scip_head_len(scip: &str) -> Option<usize> {
    let bytes = scip.as_bytes();
    let mut spaces_seen = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b' ' {
            spaces_seen += 1;
            if spaces_seen == 4 {
                return Some(i + 1);
            }
        }
    }
    None
}

/// Intern a SCIP symbol AND, when the id was newly allocated, buffer a
/// minimal stub `SymbolRecord` derived from the SCIP string itself.
///
/// Solves the cross-document reference problem on the SCIP path:
/// `derive_edges_for_document` interns target symbols (e.g. a callee
/// defined in another crate) that may never appear in the SCIP file
/// being transformed. Without a stub, the aggregation pass drops those
/// edges because `aggregate_of` has no entry for the target id; with
/// the stub, the symbol participates as its own aggregate. The full
/// `SymbolRecord` (when later seen) calls [`IdRegistry::mark_full_emitted`]
/// to drop the stub.
///
/// Mirrors the JSONL path's `buffer_stub` + `flush_registry_stubs`
/// flow that kenn-dotnet relies on for the same reason.
pub fn intern_symbol_with_stub(
    registry: &mut IdRegistry,
    language: Language,
    scip_symbol: &str,
) -> ShortId {
    if let Some(existing) = registry.lookup_symbol(language, scip_symbol) {
        return existing;
    }
    let id = registry.intern_symbol(language, scip_symbol);
    if let Some(stub) = build_stub_from_scip(language, scip_symbol, id) {
        registry.buffer_stub(id, stub);
    }
    id
}

/// Best-effort stub: parse the descriptor's last segment for `Kind` and
/// display name. Returns `None` for SCIP symbols that have no
/// descriptor at all (e.g. the bare package head `scip-ts npm pkg v1`) —
/// those represent the package itself and would clutter the symbols
/// table with phantom rows. Local symbols (`local 1`) and pseudo
/// symbols are filtered upstream.
fn build_stub_from_scip(
    language: Language,
    scip_symbol: &str,
    short_id: ShortId,
) -> Option<SymbolRecord> {
    use kenn_model::kind_classifier::kind_from_descriptor_suffix;
    let descriptor = strip_scip_head(scip_symbol);
    if descriptor.is_empty() {
        return None;
    }
    let kind = kind_from_descriptor_suffix(descriptor).unwrap_or(Kind::Variable);
    let name = derive_display_name(scip_symbol)
        .map_or_else(|| derive_short_name(descriptor), |n| derive_short_name(&n));
    // Transform the SCIP symbol into its canonical public id (e.g.
    // `go:context.Background`). A stub is a cross-document / external edge
    // target we only *reference* here; interning is keyed by the SCIP symbol
    // string, so when a real definition is seen elsewhere it resolves to the
    // same id and clears this stub. But for a genuine external (referenced,
    // never defined in-workspace — stdlib, a dependency) the stub is the
    // final record, and its `pub_id` must be the proper public id, not the
    // raw descriptor (`context/Background().`). Fall back to the descriptor
    // only if the transformer rejects the symbol.
    let raw_pub_id = super::transformer_for(language)
        .and_then(|t| t.scip_to_public(scip_symbol).ok())
        .map_or_else(|| descriptor.to_string(), kenn_model::PublicId::into_string);
    let pub_id = crate::pubid::render(language, &raw_pub_id);
    Some(SymbolRecord {
        id: short_id,
        pub_id,
        language,
        pkg_id: 0,
        kind,
        name,
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    })
}

/// Given a SCIP symbol string, return its parent SCIP symbol — same
/// `<head>` with the last descriptor segment dropped. Returns `None`
/// when there's no descriptor or only one segment (no enclosing
/// parent). Used to set `SymbolRecord.enclosing_symbol` on the SCIP
/// path, where the JSONL path's explicit parent field isn't available.
#[must_use]
pub fn parent_scip_symbol(scip_symbol: &str) -> Option<String> {
    let head_len = scip_head_len(scip_symbol)?;
    let descriptor = scip_symbol.get(head_len..)?;
    let parent_desc = kenn_model::id::descriptor::descriptor_parent(descriptor)?;
    let mut out = String::with_capacity(head_len + parent_desc.len());
    out.push_str(scip_symbol.get(..head_len)?);
    out.push_str(parent_desc);
    Some(out)
}

pub(crate) fn derive_display_name(scip_symbol: &str) -> Option<String> {
    use kenn_model::id::descriptor::Segment;
    // The display name is the last name-bearing descriptor segment.
    let descriptor = strip_scip_head(scip_symbol);
    if descriptor.is_empty() {
        return None;
    }
    let segs = kenn_model::id::descriptor::parse_descriptor(descriptor).ok()?;
    segs.iter().next_back().map(|s| match s {
        Segment::Namespace(n)
        | Segment::Type(n)
        | Segment::Term(n)
        | Segment::Macro(n)
        | Segment::Meta(n) => (*n).to_string(),
        Segment::Method { name, .. } | Segment::TypeParam(name) | Segment::Parameter(name) => {
            (*name).to_string()
        }
    })
}

pub(crate) fn derive_short_name(display_name: &str) -> String {
    // SCIP escapes a descriptor name that isn't a bare identifier by wrapping it
    // in backticks — Rust generics/lifetimes arrive as `` `StreamState<'ws>` ``.
    // Those backticks are SCIP escaping syntax, never part of the name, so the
    // SCIP ingester unwraps them here (the store then debug-asserts the DB
    // invariant that no name carries a backtick). A real identifier never
    // contains a literal backtick, so removing every backtick is the correct
    // unwrap for this grammar.
    if display_name.contains('`') {
        display_name.replace('`', "")
    } else {
        display_name.into()
    }
}

/// Per-language heuristic: does a symbol's public id name a test symbol?
///
/// Catches the case where tests live in the same source file as production
/// code (the canonical Rust `#[cfg(test)] mod tests { … }` pattern), which
/// file-path globs can't see. Cooperates with the file-glob check upstream
/// — symbols are tagged `test = true` if EITHER signal fires.
///
/// Language rules:
/// * **Rust**: any non-leaf `::`-segment named exactly `tests`, `test`,
///   `bench`, or `benches` marks the symbol (covers methods/fields/types
///   inside a `mod tests`). The leaf segment matches only when `kind` is
///   module-like — so `mod tests` itself is tagged but `FileRecord::test`
///   (a field named `test`) is not.
/// * **Go**: any `/`-or-`.`-segment ending in `_test` (or equal to
///   `test`/`tests`) — matches the `_test.go` convention even when files
///   weren't globbed.
/// * **Python / TypeScript**: no descriptor signal — file globs are the
///   primary detector; this returns `false` and lets the path check decide.
#[must_use]
pub fn is_test_descriptor(language: Language, kind: Kind, public_id: &str) -> bool {
    let Some((_lang, native)) = public_id.split_once(':') else {
        return false;
    };
    match language {
        Language::Rust => {
            let segs: Vec<&str> = native
                .split("::")
                .map(|seg| {
                    seg.trim_end_matches('/')
                        .trim_end_matches('.')
                        .trim_end_matches('#')
                })
                .collect();
            if segs.is_empty() {
                return false;
            }
            let last_idx = segs.len() - 1;
            for (i, seg) in segs.iter().enumerate() {
                let is_match = matches!(*seg, "tests" | "test" | "bench" | "benches");
                if !is_match {
                    continue;
                }
                // Non-leaf hit is always meaningful.
                if i < last_idx {
                    return true;
                }
                // Leaf hit only counts when the symbol itself is module-like
                // — guards against fields/fns named `test` in production code.
                if kind.is_scope() {
                    return true;
                }
            }
            false
        }
        Language::Go => native
            .split(['/', '.'])
            .any(|seg| seg.ends_with("_test") || seg == "tests" || seg == "test"),
        Language::Python => is_test_descriptor_python(kind, native),
        // Markdown, stylesheets, and HTML have no test-descriptor convention.
        // Swift's test flag is set by the sidecar from the SwiftPM target kind,
        // not inferred from the descriptor here.
        Language::TypeScript
        | Language::Csharp
        | Language::Swift
        | Language::Markdown
        | Language::Css
        | Language::Sass
        | Language::Html
        | Language::Text => false,
    }
}

/// Python descriptor-level test heuristic. See `is_test_descriptor` for
/// the contract. Rules (short-circuit on first match):
///
/// 1. Segment ∈ {`tests`, `test`, `__tests__`}: non-leaf unconditional;
///    leaf only when `kind.is_scope()`.
/// 2. Segment starts with `test_` — any position.
/// 3. Segment ends with `_test`: non-leaf unconditional; leaf only when
///    `kind.is_scope()` (symmetric to rule 1).
/// 4. Leaf == `conftest`.
/// 5. Leaf class shape (`Test*` / `*Test` / `*TestCase`) AND
///    `kind.is_class_like()`.
fn is_test_descriptor_python(kind: Kind, native: &str) -> bool {
    let segs: Vec<&str> = native.split('.').collect();
    if segs.is_empty() {
        return false;
    }
    let last_idx = segs.len() - 1;
    for (i, seg) in segs.iter().enumerate() {
        let is_leaf = i == last_idx;
        // Rule 1.
        if matches!(*seg, "tests" | "test" | "__tests__") && (!is_leaf || kind.is_scope()) {
            return true;
        }
        // Rule 2.
        if seg.starts_with("test_") {
            return true;
        }
        // Rule 3.
        if seg.ends_with("_test") && (!is_leaf || kind.is_scope()) {
            return true;
        }
    }
    // `segs.get(last_idx)` is always Some here — last_idx came from
    // `segs.len() - 1` after the `is_empty` early-return above — but we
    // pattern-match instead of indexing to keep `clippy::indexing_slicing`
    // satisfied. The else-arm is unreachable in practice.
    let Some(leaf) = segs.get(last_idx).copied() else {
        return false;
    };
    // Rule 4.
    if leaf == "conftest" {
        return true;
    }
    // Rule 5.
    if kind.is_class_like()
        && (leaf.starts_with("Test") || leaf.ends_with("TestCase") || leaf.ends_with("Test"))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_test_descriptor_rust_module_segment() {
        // `#[cfg(test)] mod tests { fn foo() {} }` produces a pub_id whose
        // path contains a `tests` segment.
        assert!(is_test_descriptor(
            Language::Rust,
            Kind::Function,
            "rs:kenn_indexer::transform::tests::pub_id_round_trip",
        ));
        // The module itself — kind is module-like so leaf match is allowed.
        assert!(is_test_descriptor(
            Language::Rust,
            Kind::Module,
            "rs:kenn_indexer::transform::tests/",
        ));
        // `test` (singular) — also a Rust convention.
        assert!(is_test_descriptor(
            Language::Rust,
            Kind::Function,
            "rs:my_crate::test::helper",
        ));
        // `bench`/`benches` for criterion-style benches.
        assert!(is_test_descriptor(
            Language::Rust,
            Kind::Function,
            "rs:my_crate::benches::bench_one",
        ));
    }

    #[test]
    fn is_test_descriptor_rust_does_not_match_leaf_names() {
        // A field named `test` in production code (e.g. `FileRecord.test: bool`).
        assert!(!is_test_descriptor(
            Language::Rust,
            Kind::Field,
            "rs:kenn_model::record::FileRecord::test",
        ));
        // A `pub fn test()` at module root.
        assert!(!is_test_descriptor(
            Language::Rust,
            Kind::Function,
            "rs:my_crate::test"
        ));
        // A `pub fn run_test()` in a production module.
        assert!(!is_test_descriptor(
            Language::Rust,
            Kind::Function,
            "rs:my_crate::run_test"
        ));
        // A struct named `Tester`.
        assert!(!is_test_descriptor(
            Language::Rust,
            Kind::Struct,
            "rs:my_crate::Tester"
        ));
        // Top-level item.
        assert!(!is_test_descriptor(
            Language::Rust,
            Kind::Function,
            "rs:my_crate"
        ));
    }

    #[test]
    fn is_test_descriptor_go_matches_test_files() {
        assert!(is_test_descriptor(
            Language::Go,
            Kind::Function,
            "go:github.com/foo/bar/baz_test.TestSomething",
        ));
        assert!(!is_test_descriptor(
            Language::Go,
            Kind::Function,
            "go:github.com/foo/bar/baz.Something",
        ));
    }

    #[test]
    fn is_test_descriptor_ts_and_csharp_return_false() {
        // TS / C# rely on file-path globs only at the descriptor layer.
        assert!(!is_test_descriptor(
            Language::TypeScript,
            Kind::Function,
            "ts:src/foo.test"
        ));
        assert!(!is_test_descriptor(
            Language::Csharp,
            Kind::Class,
            "cs:Foo.Tests.Bar"
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_tests_directory_non_leaf() {
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Function,
            "py:tests.test_detect.test_handles_redirect",
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_tests_module_init_leaf_scope() {
        // tests/__init__.py module — leaf `tests`, scope kind.
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Module,
            "py:tests"
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_test_prefix_module() {
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Class,
            "py:test_detect.TestDetect",
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_conftest_fixture() {
        // Non-leaf `tests` fires rule 1 regardless of `kind`.
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Function,
            "py:tests.conftest.client_fixture",
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_conftest_module_init() {
        // Isolates rule 4: leaf is `conftest`, rules 1/2/3/5 don't fire.
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Module,
            "py:conftest"
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_test_case_class() {
        // Rule 5 ends-with `TestCase`.
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Class,
            "py:graphify.smoke.SmokeTestCase",
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_test_prefix_class_in_isolation() {
        // Isolates rule 5's starts-with branch; rules 1-4 don't fire on
        // `graphify.TestParser` (no test-dir segment, no `test_` prefix
        // on any segment, no `_test` suffix, leaf isn't `conftest`).
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Class,
            "py:graphify.TestParser",
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_foo_test_module_init() {
        // Rule 3 leaf scope-kind branch — `foo_test.py` module init.
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Module,
            "py:foo_test"
        ));
    }

    #[test]
    fn is_test_descriptor_python_marks_method_in_foo_test_module() {
        // Rule 3 non-leaf branch — method inside `foo_test.py`.
        assert!(is_test_descriptor(
            Language::Python,
            Kind::Function,
            "py:foo_test.helper_function",
        ));
    }

    #[test]
    fn is_test_descriptor_python_does_not_mark_production_field_named_test() {
        // Rule 1 leaf-scope branch requires `kind.is_scope()`; Field is not scope.
        assert!(!is_test_descriptor(
            Language::Python,
            Kind::Field,
            "py:graphify.config.test",
        ));
    }

    #[test]
    fn is_test_descriptor_python_does_not_mark_variable_ending_in_test_suffix() {
        // Rule 3 leaf branch requires `kind.is_scope()`; Variable is not scope.
        assert!(!is_test_descriptor(
            Language::Python,
            Kind::Variable,
            "py:graphify.runner.previous_test",
        ));
    }

    #[test]
    fn is_test_descriptor_python_does_not_mark_unrelated_symbols() {
        assert!(!is_test_descriptor(
            Language::Python,
            Kind::Function,
            "py:graphify.detect.detect_languages",
        ));
    }

    #[test]
    fn mark_full_emitted_drops_pending_stub_for_same_short_id() {
        // Regression: on the SCIP path, `derive_edges_for_document`
        // calls `intern_symbol_with_stub` which buffers a stub
        // SymbolRecord. When the same scip_symbol later appears as a
        // `Document.symbols` entry and a real SymbolRecord is emitted,
        // the caller invokes `mark_full_emitted(short_id)`. That MUST
        // drop the buffered stub, otherwise `flush_registry_stubs` at
        // end-of-job emits the stub and the writer panics with a D5
        // exactly-once violation.
        let mut r = IdRegistry::new(Language::Rust);
        let id = intern_symbol_with_stub(&mut r, Language::Rust, "rust-analyzer cargo k 0.1 Foo#");
        r.mark_full_emitted(id);
        let leaked: Vec<_> = r.drain_pending_stubs().collect();
        assert!(
            leaked.iter().all(|s| s.id != id),
            "mark_full_emitted must drop the stub for the same short_id",
        );
    }

    /// An external symbol (referenced, never defined in-workspace) survives
    /// as a drained stub. Its `pub_id` MUST be the canonical public id, not
    /// the raw SCIP descriptor — otherwise edges point at `context/Background().`
    /// instead of `go:context.Background`.
    #[test]
    fn external_stub_pub_id_is_transformed_not_raw_descriptor() {
        let mut r = IdRegistry::new(Language::Go);
        let id = intern_symbol_with_stub(
            &mut r,
            Language::Go,
            "scip-go gomod github.com/golang/go/src go1.20 context/Background().",
        );
        let stubs: Vec<_> = r.drain_pending_stubs().collect();
        let stub = stubs.iter().find(|s| s.id == id).expect("stub buffered");
        assert_eq!(stub.pub_id, "go:context.Background");
    }

    #[test]
    fn strip_scip_head_returns_descriptor() {
        assert_eq!(
            strip_scip_head("rust-analyzer cargo k 0.1 foo/bar."),
            "foo/bar."
        );
        assert_eq!(strip_scip_head("not enough"), "");
    }

    #[test]
    fn derive_display_name_picks_last_named_segment() {
        assert_eq!(
            derive_display_name("rust-analyzer cargo k 0.1 a/b/Foo#bar()."),
            Some("bar".into())
        );
        assert_eq!(
            derive_display_name("rust-analyzer cargo k 0.1 println!"),
            Some("println".into())
        );
    }

    #[test]
    fn derive_short_name_unwraps_scip_backtick_escaping() {
        // SCIP wraps a non-identifier descriptor name (Rust generics/lifetimes)
        // in backticks; the ingester unwraps them so no name carries a backtick
        // into the store (which debug-asserts the invariant).
        assert_eq!(derive_short_name("`StreamState<'ws>`"), "StreamState<'ws>");
        assert_eq!(derive_short_name("`Walker<'_>`"), "Walker<'_>");
        // A bare identifier is untouched.
        assert_eq!(derive_short_name("plain"), "plain");
    }
}
