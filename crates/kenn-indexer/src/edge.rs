//! Edge derivation (section 5 of the proposal).
//!
//! For each occurrence in a SCIP `Document`, the producer needs to attribute
//! the edge to a FROM symbol — the function/method/initializer in whose
//! body the reference lives. SCIP gives us this directly (when populated)
//! via `Occurrence.enclosing_range`; otherwise we fall back to the
//! [`crate::enclosing`] provider chain (section 5b).
//!
//! Edge derivation produces *pair-deduplicated* edges per source-data-model
//! D10: `(from, to, kind)` is unique per run. We accumulate into a
//! `HashSet`, not a `Vec`, so duplicate occurrences (the common case for
//! method calls) collapse.

use std::collections::HashSet;

use kenn_model::{EdgeKind, EdgeProperties, EdgeRecord, FieldOp, Language, ShortId};
use protobuf::Enum;
use scip::types::{symbol_information::Kind as ScipKind, Document, Occurrence, SymbolRole};

use crate::enclosing::{EnclosingProvider, OccurrenceLocator};
use crate::transform::IdRegistry;

/// SCIP role bitmask helpers.
fn has_role(occ: &Occurrence, role: SymbolRole) -> bool {
    (occ.symbol_roles & (role as i32)) != 0
}

#[must_use]
pub fn is_definition(occ: &Occurrence) -> bool {
    has_role(occ, SymbolRole::Definition)
}

#[must_use]
pub fn is_pseudo_symbol(symbol: &str) -> bool {
    symbol.starts_with("local ")
}

/// Per-document def-range index. Keyed by `Range = [start_line, start_col,
/// end_line, end_col]`. SCIP always emits ranges as `Vec<i32>`.
#[derive(Debug, Default)]
pub struct DocumentDefIndex {
    /// `(start_line, start_col, end_line, end_col)` and the SCIP symbol that
    /// owns the range.
    by_position: Vec<(Range4, String)>,
}

pub type Range4 = (i32, i32, i32, i32);

/// Parse a SCIP range slice into a `Range4`. SCIP encodes a range as
/// `[start_line, start_col, end_col]` (3-int single-line) or `[start_line,
/// start_col, end_line, end_col]`. Returns `None` for any other length —
/// empty (field absent) or malformed.
fn parse_range4(span: &[i32]) -> Option<Range4> {
    match *span {
        [sl, sc, ec] => Some((sl, sc, sl, ec)),
        [sl, sc, el, ec] => Some((sl, sc, el, ec)),
        _ => None,
    }
}

impl DocumentDefIndex {
    pub fn from_document(doc: &Document) -> Self {
        let mut by_position: Vec<(Range4, String)> = Vec::new();
        for occ in &doc.occurrences {
            if !is_definition(occ) || is_pseudo_symbol(&occ.symbol) {
                continue;
            }
            // Index a definition by its `enclosing_range` (the body extent)
            // when the indexer populated it, falling back to the name-token
            // `range`. A reference inside a function body is contained by the
            // body, never by the bare name identifier — so FROM-attribution
            // (`smallest_enclosing`) must span the body. SCIP indexers that
            // follow the spec (e.g. scip-go) emit `enclosing_range` on
            // container definitions and omit it on leaf defs (fields, consts,
            // vars), which correctly fall back to `range`. Without this,
            // languages whose indexer puts `enclosing_range` only on
            // definitions (not on reference occurrences) get almost no
            // occurrence edges, because name ranges never contain references.
            //
            // Both fields share `range`'s encoding: `[start_line, start_col,
            // end_col]` (3-int single-line) or `[start_line, start_col,
            // end_line, end_col]`. Prefer `enclosing_range`; fall back to the
            // name `range` when it is absent OR malformed, so a definition is
            // never dropped from the attribution index.
            let Some(r) = parse_range4(&occ.enclosing_range).or_else(|| parse_range4(&occ.range))
            else {
                continue;
            };
            by_position.push((r, occ.symbol.clone()));
        }
        by_position.sort_by_key(|(r, _)| *r);
        Self { by_position }
    }

    /// Find the smallest def range that contains `(line, col)`. Smallest =
    /// the def whose range `(start, end)` spans `(line, col)` AND has no
    /// strictly-smaller-range def also containing the point.
    #[must_use]
    pub fn smallest_enclosing(&self, line: i32, col: i32) -> Option<&str> {
        let mut best: Option<(Range4, &str)> = None;
        for (r, sym) in &self.by_position {
            if !range_contains(*r, line, col) {
                continue;
            }
            if let Some((cur, _)) = best {
                if range_size(*r) < range_size(cur) {
                    best = Some((*r, sym));
                }
            } else {
                best = Some((*r, sym));
            }
        }
        best.map(|(_, s)| s)
    }
}

fn range_contains(r: Range4, line: i32, col: i32) -> bool {
    let (sl, sc, el, ec) = r;
    if line < sl || line > el {
        return false;
    }
    if line == sl && col < sc {
        return false;
    }
    if line == el && col > ec {
        return false;
    }
    true
}

fn range_size(r: Range4) -> i64 {
    let (sl, sc, el, ec) = r;
    let dl = i64::from(el - sl);
    if dl == 0 {
        i64::from(ec - sc)
    } else {
        dl * 1_000_000 + i64::from(ec - sc)
    }
}

/// Per-document edge classification.
///
/// `def_counts` returns the number of times the *target* symbol is defined
/// across the workspace. Targets with `def_count > 1` are dropped — that
/// arm filters crate-root markers (rust-analyzer emits one per source file)
/// and producer-side duplication patterns. Targets with `def_count == 0`
/// are emitted as user→external edges; the target symbol is interned via
/// `intern_symbol_with_stub`, lands in `pending_stub_records`, and is
/// drained at end-of-job by `flush_registry_stubs` with `external = true`.
pub fn derive_edges_for_document<E: EnclosingProvider>(
    doc: &Document,
    workspace_path: &str,
    enclosing: &mut E,
    registry: &mut IdRegistry,
    language: Language,
    def_counts: &impl Fn(&str) -> usize,
    out: &mut HashSet<EdgeRecord, impl std::hash::BuildHasher>,
) {
    let def_index = DocumentDefIndex::from_document(doc);
    // SCIP encodes the callee's "kind" (method/function/constructor/...)
    // only on `SymbolInformation`, not on the occurrence at the call
    // site. Build a per-document lookup so the kind-hint refiner can
    // promote a default `TypeUse` to `Calls` / `Instantiates` when the
    // target's shape says so. Cross-document targets miss this map and
    // fall back to descriptor-suffix inference (`().` → calls).
    let target_kinds: std::collections::HashMap<&str, ScipKind> = doc
        .symbols
        .iter()
        .filter_map(|s| ScipKind::from_i32(s.kind.value()).map(|k| (s.symbol.as_str(), k)))
        // (protobuf::Enum brings from_i32 into scope above)
        .collect();

    for occ in &doc.occurrences {
        if is_definition(occ) || is_pseudo_symbol(&occ.symbol) {
            continue;
        }
        let target_def_count = def_counts(&occ.symbol);
        if target_def_count > 1 {
            continue;
        }
        let ([sl, sc, _] | [sl, sc, _, _]) = *occ.range.as_slice() else {
            continue;
        };
        let (line, col) = (sl, sc);
        let Some(enclosing_symbol) = enclosing.attribute_from(
            workspace_path,
            line,
            col,
            &def_index,
            &OccurrenceLocator { occurrence: occ },
        ) else {
            continue;
        };
        if enclosing_symbol == occ.symbol {
            // self-reference inside the def's body — skip; producers don't emit a self-edge
            continue;
        }
        // Use the stub-buffering intern so cross-document edge targets
        // (e.g. a callee defined in another crate that this document
        // only references) get a minimal SymbolRecord, instead of
        // sitting as an orphan id that the aggregation pass drops.
        let from_id =
            crate::transform::intern_symbol_with_stub(registry, language, &enclosing_symbol);
        let to_id = crate::transform::intern_symbol_with_stub(registry, language, &occ.symbol);
        let target_kind = target_kinds
            .get(occ.symbol.as_str())
            .copied()
            .unwrap_or(ScipKind::UnspecifiedKind);
        let kind = refine_with_kind_hints(classify_edge_kind(occ), &occ.symbol, target_kind);
        out.insert(EdgeRecord {
            src_id: from_id,
            target_id: to_id,
            properties: edge_properties(kind, occ),
        });
    }
}

/// Task 5.3 — classify the SCIP occurrence into a source-data-model `EdgeKind`.
#[must_use]
pub fn classify_edge_kind(occ: &Occurrence) -> EdgeKind {
    // A write is unambiguously a field / variable write.
    if has_role(occ, SymbolRole::WriteAccess) {
        return EdgeKind::FieldAccess;
    }
    if has_role(occ, SymbolRole::Import) {
        return EdgeKind::Imports;
    }
    // A read is a field access ONLY when the target is a data member
    // (field / const / variable). Some indexers — notably scip-go — tag
    // EVERY reference (calls and type uses included) as ReadAccess, so the
    // role alone can't distinguish them; the target's descriptor does.
    // Method / function and type targets fall through to the default
    // `TypeUse`, which `refine_with_kind_hints` then promotes (`().`
    // callable → `Calls`; a type stays `TypeUse`).
    if has_role(occ, SymbolRole::ReadAccess) && target_is_data_symbol(&occ.symbol) {
        return EdgeKind::FieldAccess;
    }
    // Default: pure references that aren't field accesses get classified
    // as `TypeUse` rather than the now-removed `references` catch-all.
    // The `instantiates` and `calls` distinctions need either syntactic
    // hints (preceding `new`, parens) or descriptor-suffix inference,
    // both done in [`refine_with_kind_hints`] below.
    EdgeKind::TypeUse
}

/// True when a SCIP symbol denotes a data member — a field, constant, or
/// variable: a `term` descriptor ending in `.` that is not a `()` callable.
/// Methods (`().`), types (`#`), and namespaces (`/`) are not data symbols.
/// A symbol with no parseable descriptor conservatively counts as data, so a
/// bare read stays a field access.
fn target_is_data_symbol(scip: &str) -> bool {
    match scip_descriptor_suffix_char(scip) {
        Some('.') => !scip.contains("()."),
        Some(_) => false,
        None => true,
    }
}

fn edge_properties(kind: EdgeKind, occ: &Occurrence) -> EdgeProperties {
    match kind {
        EdgeKind::FieldAccess => {
            let op = if has_role(occ, SymbolRole::WriteAccess) {
                FieldOp::Write
            } else {
                FieldOp::Read
            };
            EdgeProperties::FieldAccess { op }
        }
        EdgeKind::Imports => EdgeProperties::Imports {
            kind: kenn_model::ImportKind::Explicit,
        },
        EdgeKind::TypeUse => EdgeProperties::TypeUse,
        EdgeKind::Calls => EdgeProperties::Calls,
        EdgeKind::Instantiates => EdgeProperties::Instantiates,
        EdgeKind::Implements => EdgeProperties::Implements,
        EdgeKind::Overrides => EdgeProperties::Overrides,
        EdgeKind::GenericConstraint => EdgeProperties::GenericConstraint,
        EdgeKind::DefinedIn => EdgeProperties::DefinedIn,
        EdgeKind::Contains => EdgeProperties::Contains,
        EdgeKind::CorrespondsTo => EdgeProperties::CorrespondsTo {
            source: kenn_model::IsomorphismSource::Config,
            generator: String::new(),
            canonical: 0,
        },
        // Markdown link/embed edges and CSS `uses_class` edges are produced by
        // their own producers, not derived from SCIP occurrences, so they never
        // reach this classifier.
        #[expect(
            clippy::unreachable,
            reason = "markdown/css producer edges never come from a SCIP occurrence"
        )]
        EdgeKind::LinksTo
        | EdgeKind::Embeds
        | EdgeKind::LinksToFile
        | EdgeKind::UsesCssClass
        | EdgeKind::ExtendsRule
        | EdgeKind::DefinesTable
        | EdgeKind::AltersTable
        | EdgeKind::AccessesTable
        | EdgeKind::ExtendsType => {
            unreachable!("producer-emitted {kind:?} edges are not derived from SCIP occurrences")
        }
    }
}

/// Refine the default `TypeUse` to `Calls` / `Instantiates` when the target
/// symbol's descriptor reveals callable shape. Used when no syntactic
/// preceding-`new`/parens hints are available from SCIP.
#[must_use]
pub fn refine_with_kind_hints(
    default: EdgeKind,
    target_scip: &str,
    target_kind: ScipKind,
) -> EdgeKind {
    if default != EdgeKind::TypeUse {
        return default;
    }
    if matches!(
        target_kind,
        ScipKind::Method
            | ScipKind::Function
            | ScipKind::StaticMethod
            | ScipKind::AbstractMethod
            | ScipKind::ProtocolMethod
            | ScipKind::PureVirtualMethod
    ) {
        return EdgeKind::Calls;
    }
    if matches!(target_kind, ScipKind::Constructor) {
        return EdgeKind::Instantiates;
    }
    // Fallback to descriptor parse: a `().` last segment indicates callable.
    if let Some(suffix) = scip_descriptor_suffix_char(target_scip) {
        if suffix == '.' && target_scip.contains("().") {
            return EdgeKind::Calls;
        }
    }
    default
}

fn scip_descriptor_suffix_char(scip: &str) -> Option<char> {
    let mut parts = scip.splitn(5, ' ');
    let descriptor = parts.nth(4)?;
    descriptor.chars().last()
}

#[must_use]
pub fn drop_short_id_suppress(id: ShortId) -> ShortId {
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types::SymbolRole;

    fn occ(symbol: &str, range: Vec<i32>, roles: i32) -> Occurrence {
        let mut o = Occurrence::new();
        o.symbol = symbol.into();
        o.range = range;
        o.symbol_roles = roles;
        o
    }

    #[test]
    fn def_index_finds_smallest_enclosing() {
        let mut doc = Document::new();
        let outer = occ(
            "rust-analyzer cargo k 0.1 m/Outer#",
            vec![0, 0, 99, 0],
            SymbolRole::Definition as i32,
        );
        let inner = occ(
            "rust-analyzer cargo k 0.1 m/Outer#inner().",
            vec![10, 0, 30, 0],
            SymbolRole::Definition as i32,
        );
        doc.occurrences = vec![outer, inner];
        let idx = DocumentDefIndex::from_document(&doc);
        assert_eq!(
            idx.smallest_enclosing(15, 5),
            Some("rust-analyzer cargo k 0.1 m/Outer#inner().")
        );
        assert_eq!(
            idx.smallest_enclosing(95, 0),
            Some("rust-analyzer cargo k 0.1 m/Outer#")
        );
        assert_eq!(idx.smallest_enclosing(120, 0), None);
    }

    /// scip-go puts `enclosing_range` (the function body) on container
    /// definitions but leaves the name-token `range` tiny. A reference inside
    /// the body — outside the name range — must still attribute to the
    /// function. Indexing by name range alone (the prior behavior) returned
    /// `None` here, which is why Go produced almost no occurrence edges.
    #[test]
    fn def_index_uses_enclosing_range_for_body_containment() {
        let mut doc = Document::new();
        let mut func = occ(
            "scip-go gomod m v0 `m`/DoWork().",
            // Name token: line 5, cols 5..11 — does NOT contain line 8.
            vec![5, 5, 5, 11],
            SymbolRole::Definition as i32,
        );
        // Body spans lines 5..20 — DOES contain a reference on line 8.
        func.enclosing_range = vec![5, 0, 20, 0];
        doc.occurrences = vec![func];
        let idx = DocumentDefIndex::from_document(&doc);
        assert_eq!(
            idx.smallest_enclosing(8, 4),
            Some("scip-go gomod m v0 `m`/DoWork()."),
            "reference inside the body must attribute to the function",
        );
    }

    /// A malformed (wrong-length) `enclosing_range` must NOT drop the def
    /// from the index — it falls back to the name `range`.
    #[test]
    fn def_index_falls_back_to_name_range_on_malformed_enclosing() {
        let mut doc = Document::new();
        let mut func = occ(
            "scip-go gomod m v0 `m`/DoWork().",
            vec![5, 0, 5, 20],
            SymbolRole::Definition as i32,
        );
        func.enclosing_range = vec![5, 0]; // malformed (2 ints)
        doc.occurrences = vec![func];
        let idx = DocumentDefIndex::from_document(&doc);
        assert_eq!(
            idx.smallest_enclosing(5, 10),
            Some("scip-go gomod m v0 `m`/DoWork()."),
            "malformed enclosing_range must fall back to the name range",
        );
    }

    #[test]
    fn pseudo_symbol_filtered() {
        assert!(is_pseudo_symbol("local 4"));
        assert!(!is_pseudo_symbol("rust-analyzer cargo k 0.1 foo."));
    }

    /// scip-go tags every reference as `ReadAccess`. The edge kind must come
    /// from the target descriptor, not the role: a read of a `().` method is
    /// a call, a read of a `#` type is a type-use, and only a read of a `.`
    /// term (field/const/var) is a field access.
    #[test]
    fn read_access_classified_by_target_descriptor() {
        let method = occ(
            "scip-go gomod m v0 `m`/Server#Start().",
            vec![1, 0, 1, 5],
            SymbolRole::ReadAccess as i32,
        );
        let kind = refine_with_kind_hints(
            classify_edge_kind(&method),
            &method.symbol,
            ScipKind::UnspecifiedKind,
        );
        assert_eq!(kind, EdgeKind::Calls, "read of a method() is a call");

        let typ = occ(
            "scip-go gomod m v0 `m`/Config#",
            vec![1, 0, 1, 5],
            SymbolRole::ReadAccess as i32,
        );
        assert_eq!(
            classify_edge_kind(&typ),
            EdgeKind::TypeUse,
            "read of a type is a type-use",
        );

        let field = occ(
            "scip-go gomod m v0 `m`/Config#Name.",
            vec![1, 0, 1, 5],
            SymbolRole::ReadAccess as i32,
        );
        assert_eq!(
            classify_edge_kind(&field),
            EdgeKind::FieldAccess,
            "read of a struct field is a field access",
        );
    }

    #[test]
    fn classify_field_access_distinguishes_read_write() {
        let read = occ("x", vec![0, 0, 0, 1], SymbolRole::ReadAccess as i32);
        let write = occ("x", vec![0, 0, 0, 1], SymbolRole::WriteAccess as i32);
        match edge_properties(classify_edge_kind(&read), &read) {
            EdgeProperties::FieldAccess { op } => assert_eq!(op, FieldOp::Read),
            other => panic!("{other:?}"),
        }
        match edge_properties(classify_edge_kind(&write), &write) {
            EdgeProperties::FieldAccess { op } => assert_eq!(op, FieldOp::Write),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn classify_import_role() {
        let imp = occ("x", vec![0, 0, 0, 1], SymbolRole::Import as i32);
        assert_eq!(classify_edge_kind(&imp), EdgeKind::Imports);
    }

    #[test]
    fn refinement_promotes_typeuse_to_calls_via_descriptor() {
        let kind = refine_with_kind_hints(
            EdgeKind::TypeUse,
            "rust-analyzer cargo k 0.1 m/foo().",
            ScipKind::UnspecifiedKind,
        );
        assert_eq!(kind, EdgeKind::Calls);
    }

    #[test]
    fn refinement_promotes_typeuse_to_instantiates_for_constructor() {
        let kind = refine_with_kind_hints(
            EdgeKind::TypeUse,
            "rust-analyzer cargo k 0.1 m/Foo#`<init>`().",
            ScipKind::Constructor,
        );
        assert_eq!(kind, EdgeKind::Instantiates);
    }

    /// Build a synthetic `Document` with one definition + one
    /// occurrence pointing to a target defined exactly once.
    fn doc_with_def_and_ref(from: &str, to: &str) -> Document {
        let mut doc = Document::new();
        doc.relative_path = "src/lib.rs".into();
        let def = occ(from, vec![0, 0, 99, 0], SymbolRole::Definition as i32);
        let target_def = occ(to, vec![5, 0, 9, 0], SymbolRole::Definition as i32);
        // Occurrence of `to` inside the `from`'s body, no roles → TypeUse.
        let usage = occ(to, vec![10, 0, 10, 5], 0);
        doc.occurrences = vec![def, target_def, usage];
        doc
    }

    /// `derive_edges_for_document` is the main per-document edge
    /// derivation pass. Cover the happy path (definition + usage →
    /// one edge emitted), the skip-self-reference branch, and the
    /// skip-zero-or-multiple-defs filter.
    #[test]
    fn derive_edges_for_document_emits_one_edge_for_unique_target() {
        use crate::enclosing::BareLastPrecedingDef;
        use crate::transform::IdRegistry;
        use std::collections::{HashMap, HashSet};

        let doc = doc_with_def_and_ref(
            "rust-analyzer cargo k 0.1 m/Caller#",
            "rust-analyzer cargo k 0.1 m/Callee#",
        );
        let mut enclosing = BareLastPrecedingDef;
        let mut registry = IdRegistry::new(kenn_model::Language::Rust);
        let mut def_counts: HashMap<String, usize> = HashMap::new();
        def_counts.insert("rust-analyzer cargo k 0.1 m/Caller#".into(), 1);
        def_counts.insert("rust-analyzer cargo k 0.1 m/Callee#".into(), 1);
        let count_fn = |s: &str| def_counts.get(s).copied().unwrap_or(0);
        let mut out: HashSet<EdgeRecord> = HashSet::new();
        derive_edges_for_document(
            &doc,
            "/ws/src/lib.rs",
            &mut enclosing,
            &mut registry,
            kenn_model::Language::Rust,
            &count_fn,
            &mut out,
        );
        assert_eq!(out.len(), 1, "expected one derived edge, got {out:?}");
    }

    #[test]
    fn derive_edges_for_document_skips_when_target_has_multiple_defs() {
        use crate::enclosing::BareLastPrecedingDef;
        use crate::transform::IdRegistry;
        use std::collections::HashSet;

        let doc = doc_with_def_and_ref(
            "rust-analyzer cargo k 0.1 m/Caller#",
            "rust-analyzer cargo k 0.1 m/Callee#",
        );
        let mut enclosing = BareLastPrecedingDef;
        let mut registry = IdRegistry::new(kenn_model::Language::Rust);
        // Multiply-defined target: still skipped — the `>1` arm filters
        // crate-root markers and known producer duplication patterns.
        let count_many = |_: &str| 2_usize;
        let mut out: HashSet<EdgeRecord> = HashSet::new();
        derive_edges_for_document(
            &doc,
            "/ws/src/lib.rs",
            &mut enclosing,
            &mut registry,
            kenn_model::Language::Rust,
            &count_many,
            &mut out,
        );
        assert!(out.is_empty(), "target def_count>1 must drop all edges");
    }

    #[test]
    fn derive_edges_for_document_emits_when_target_has_zero_defs() {
        use crate::enclosing::BareLastPrecedingDef;
        use crate::transform::IdRegistry;
        use std::collections::HashSet;

        let doc = doc_with_def_and_ref(
            "rust-analyzer cargo k 0.1 m/Caller#",
            // Target with no workspace definition — e.g. a stdlib call.
            "rust-analyzer cargo core 0.0 m/Result#unwrap().",
        );
        let mut enclosing = BareLastPrecedingDef;
        let mut registry = IdRegistry::new(kenn_model::Language::Rust);
        // Caller is defined locally; callee has zero workspace defs (extern).
        let count_fn = |s: &str| usize::from(s == "rust-analyzer cargo k 0.1 m/Caller#");
        let mut out: HashSet<EdgeRecord> = HashSet::new();
        derive_edges_for_document(
            &doc,
            "/ws/src/lib.rs",
            &mut enclosing,
            &mut registry,
            kenn_model::Language::Rust,
            &count_fn,
            &mut out,
        );
        assert_eq!(
            out.len(),
            1,
            "target def_count=0 must emit a user→external edge"
        );
    }

    /// `edge_properties` (the `edge.rs` variant — takes an `EdgeKind`
    /// and an occurrence) covers every `EdgeKind` arm. `FieldAccess`
    /// is subdispatched on the occurrence's read/write role; both
    /// paths are exercised.
    #[test]
    fn edge_properties_from_kind_and_occ_covers_every_arm() {
        let neutral = occ("x", vec![0, 0, 0, 1], 0);
        // Read/write are the two branches inside FieldAccess.
        let read = occ("x", vec![0, 0, 0, 1], SymbolRole::ReadAccess as i32);
        let write = occ("x", vec![0, 0, 0, 1], SymbolRole::WriteAccess as i32);

        assert!(matches!(
            edge_properties(EdgeKind::FieldAccess, &read),
            EdgeProperties::FieldAccess { op: FieldOp::Read }
        ));
        assert!(matches!(
            edge_properties(EdgeKind::FieldAccess, &write),
            EdgeProperties::FieldAccess { op: FieldOp::Write }
        ));
        // No role => Read (the else-branch).
        assert!(matches!(
            edge_properties(EdgeKind::FieldAccess, &neutral),
            EdgeProperties::FieldAccess { op: FieldOp::Read }
        ));

        assert!(matches!(
            edge_properties(EdgeKind::Imports, &neutral),
            EdgeProperties::Imports {
                kind: kenn_model::ImportKind::Explicit
            }
        ));
        assert!(matches!(
            edge_properties(EdgeKind::TypeUse, &neutral),
            EdgeProperties::TypeUse
        ));
        assert!(matches!(
            edge_properties(EdgeKind::Calls, &neutral),
            EdgeProperties::Calls
        ));
        assert!(matches!(
            edge_properties(EdgeKind::Instantiates, &neutral),
            EdgeProperties::Instantiates
        ));
        assert!(matches!(
            edge_properties(EdgeKind::Implements, &neutral),
            EdgeProperties::Implements
        ));
        assert!(matches!(
            edge_properties(EdgeKind::Overrides, &neutral),
            EdgeProperties::Overrides
        ));
        assert!(matches!(
            edge_properties(EdgeKind::GenericConstraint, &neutral),
            EdgeProperties::GenericConstraint
        ));
        assert!(matches!(
            edge_properties(EdgeKind::DefinedIn, &neutral),
            EdgeProperties::DefinedIn
        ));
        assert!(matches!(
            edge_properties(EdgeKind::Contains, &neutral),
            EdgeProperties::Contains
        ));
        assert!(matches!(
            edge_properties(EdgeKind::CorrespondsTo, &neutral),
            EdgeProperties::CorrespondsTo {
                source: kenn_model::IsomorphismSource::Config,
                ..
            }
        ));
    }
}
