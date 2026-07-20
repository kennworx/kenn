//! Translation from indexer-emitted kinds and SCIP descriptor suffixes to
//! the closed [`Kind`] enum (design D7, scip-indexing-pipeline D14).
//!
//! Two paths:
//! 1. When `SymbolInformation.kind` is set (scip-go, rust-analyzer): map via
//!    [`kind_from_scip_go_kind`] / [`kind_from_rust_analyzer_kind`].
//! 2. When unset (scip-typescript, scip-python): derive from the SCIP
//!    descriptor's last segment via [`kind_from_descriptor_suffix`].

use crate::id::descriptor::{parse_descriptor, Segment};
use crate::kind::Kind;

/// Last-segment-suffix → Kind. Returns `None` if the descriptor is empty or
/// unrecognized; callers default to [`Kind::Variable`] in that case.
#[must_use]
pub fn kind_from_descriptor_suffix(descriptor: &str) -> Option<Kind> {
    let segs = parse_descriptor(descriptor).ok()?;
    let last = segs.last()?;
    Some(match last {
        Segment::Namespace(_) => Kind::Namespace,
        Segment::Type(_) => Kind::Class,
        Segment::Term(_) => Kind::Field,
        Segment::Method { .. } => Kind::Method,
        Segment::TypeParam(_) => Kind::TypeParameter,
        Segment::Parameter(_) => Kind::Parameter,
        Segment::Meta(_) => Kind::Constant,
        Segment::Macro(_) => Kind::Macro,
    })
}

/// SCIP `SymbolInformation.kind` integer values. Mirror of the
/// `SymbolInformation_Kind` enum in the SCIP protobuf — kept here so callers
/// can map integers without importing the SCIP crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ScipKind {
    UnspecifiedKind = 0,
    AbstractMethod = 66,
    Accessor = 72,
    Array = 1,
    Assertion = 2,
    AssociatedType = 3,
    Attribute = 4,
    Axiom = 5,
    Boolean = 6,
    Class = 7,
    Constant = 8,
    Constructor = 9,
    Contract = 62,
    DataFamily = 10,
    Delegate = 73,
    Enum = 11,
    EnumMember = 12,
    Event = 13,
    Fact = 14,
    Field = 15,
    File = 16,
    Function = 17,
    Getter = 18,
    Grammar = 19,
    Instance = 20,
    Interface = 21,
    Key = 22,
    Lang = 23,
    Lemma = 24,
    Library = 64,
    Macro = 25,
    Method = 26,
    MethodAlias = 74,
    MethodReceiver = 27,
    Message = 28,
    Modifier = 65,
    Module = 29,
    Namespace = 30,
    Null = 31,
    Number = 32,
    Object = 33,
    Operator = 34,
    Package = 35,
    PackageObject = 36,
    Parameter = 37,
    ParameterLabel = 38,
    Pattern = 39,
    Predicate = 40,
    Property = 41,
    Protocol = 42,
    ProtocolMethod = 68,
    PureVirtualMethod = 69,
    Quasiquoter = 43,
    SelfParameter = 44,
    Setter = 45,
    Signature = 46,
    SingletonClass = 75,
    SingletonMethod = 76,
    StaticDataMember = 79,
    StaticEvent = 80,
    StaticField = 81,
    StaticMethod = 70,
    StaticProperty = 82,
    StaticVariable = 71,
    String = 48,
    Struct = 49,
    Subscript = 47,
    Tactic = 50,
    Theorem = 51,
    ThisParameter = 52,
    Trait = 53,
    TraitMethod = 54,
    Type = 55,
    TypeAlias = 56,
    TypeClass = 57,
    TypeClassMethod = 58,
    TypeFamily = 59,
    TypeParameter = 60,
    Union = 77,
    Value = 61,
    Variable = 63,
}

impl ScipKind {
    #[must_use]
    pub fn from_i32(v: i32) -> Option<Self> {
        // Keep this sparse-match approach to avoid hand-writing 80+ branches.
        match v {
            0 => Some(Self::UnspecifiedKind),
            7 => Some(Self::Class),
            8 => Some(Self::Constant),
            9 => Some(Self::Constructor),
            11 => Some(Self::Enum),
            12 => Some(Self::EnumMember),
            13 => Some(Self::Event),
            15 => Some(Self::Field),
            17 => Some(Self::Function),
            21 => Some(Self::Interface),
            25 => Some(Self::Macro),
            26 => Some(Self::Method),
            29 => Some(Self::Module),
            30 => Some(Self::Namespace),
            34 => Some(Self::Operator),
            35 => Some(Self::Package),
            37 => Some(Self::Parameter),
            41 => Some(Self::Property),
            49 => Some(Self::Struct),
            53 => Some(Self::Trait),
            55 => Some(Self::Type),
            56 => Some(Self::TypeAlias),
            60 => Some(Self::TypeParameter),
            63 => Some(Self::Variable),
            66 => Some(Self::AbstractMethod),
            70 => Some(Self::StaticMethod),
            72 => Some(Self::Accessor),
            _ => None,
        }
    }
}

/// scip-go mapping (D7 + scip-indexing-pipeline D14).
#[must_use]
#[expect(
    clippy::match_same_arms,
    reason = "self-documenting mapping table: arms with the same Kind list each SCIP variant explicitly so future readers see the spec"
)]
pub fn kind_from_scip_go_kind(scip: ScipKind) -> Kind {
    match scip {
        ScipKind::Package | ScipKind::PackageObject => Kind::Package,
        ScipKind::Module => Kind::Module,
        ScipKind::Namespace => Kind::Namespace,
        ScipKind::Class => Kind::Class,
        ScipKind::Struct => Kind::Struct,
        ScipKind::Interface | ScipKind::Protocol => Kind::Interface,
        ScipKind::Trait | ScipKind::TypeClass => Kind::Trait,
        ScipKind::Enum => Kind::Enum,
        ScipKind::EnumMember => Kind::EnumMember,
        ScipKind::TypeAlias | ScipKind::Type => Kind::TypeAlias,
        ScipKind::Method
        | ScipKind::AbstractMethod
        | ScipKind::ProtocolMethod
        | ScipKind::PureVirtualMethod
        | ScipKind::StaticMethod
        | ScipKind::TraitMethod
        | ScipKind::TypeClassMethod
        | ScipKind::SingletonMethod
        | ScipKind::MethodAlias
        | ScipKind::Accessor
        | ScipKind::Getter
        | ScipKind::Setter => Kind::Method,
        ScipKind::Function => Kind::Function,
        ScipKind::Constructor => Kind::Constructor,
        ScipKind::Operator => Kind::Operator,
        ScipKind::Field | ScipKind::StaticField | ScipKind::StaticDataMember => Kind::Field,
        ScipKind::Property | ScipKind::StaticProperty => Kind::Property,
        ScipKind::Constant => Kind::Constant,
        ScipKind::Variable | ScipKind::StaticVariable => Kind::Variable,
        ScipKind::Parameter
        | ScipKind::SelfParameter
        | ScipKind::ThisParameter
        | ScipKind::ParameterLabel => Kind::Parameter,
        ScipKind::TypeParameter | ScipKind::AssociatedType => Kind::TypeParameter,
        ScipKind::Macro => Kind::Macro,
        // Catch-all for kinds Go doesn't typically emit but might appear:
        _ => Kind::Variable,
    }
}

/// rust-analyzer mapping (D7 + scip-indexing-pipeline D14).
///
/// rust-analyzer's emitted kinds overlap heavily with scip-go's; the only
/// Rust-specific consideration is that `impl#[Type][Trait]` symbols come
/// through with `kind = 0` (unset) and are detected structurally by
/// [`crate::id::RustTransformer`], not here.
#[must_use]
pub fn kind_from_rust_analyzer_kind(scip: ScipKind) -> Kind {
    kind_from_scip_go_kind(scip)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 6.2 — Per-indexer kind mapping table is exhaustive on observed kinds
    /// and deterministic.
    #[test]
    fn scip_go_emitted_kinds_map_deterministically() {
        for (sk, expected) in [
            (ScipKind::Package, Kind::Package),
            (ScipKind::Module, Kind::Module),
            (ScipKind::Namespace, Kind::Namespace),
            (ScipKind::Class, Kind::Class),
            (ScipKind::Struct, Kind::Struct),
            (ScipKind::Interface, Kind::Interface),
            (ScipKind::Trait, Kind::Trait),
            (ScipKind::Enum, Kind::Enum),
            (ScipKind::EnumMember, Kind::EnumMember),
            (ScipKind::TypeAlias, Kind::TypeAlias),
            (ScipKind::Method, Kind::Method),
            (ScipKind::Function, Kind::Function),
            (ScipKind::Constructor, Kind::Constructor),
            (ScipKind::Field, Kind::Field),
            (ScipKind::Property, Kind::Property),
            (ScipKind::Constant, Kind::Constant),
            (ScipKind::Variable, Kind::Variable),
            (ScipKind::Parameter, Kind::Parameter),
            (ScipKind::TypeParameter, Kind::TypeParameter),
            (ScipKind::Macro, Kind::Macro),
            (ScipKind::Operator, Kind::Operator),
        ] {
            assert_eq!(kind_from_scip_go_kind(sk), expected, "scip-go kind {sk:?}");
            assert_eq!(
                kind_from_rust_analyzer_kind(sk),
                expected,
                "rust-analyzer kind {sk:?}"
            );
        }
    }

    /// 6.1 — Descriptor-suffix fallback (used for scip-typescript and
    /// scip-python where `SymbolInformation.kind` is unset).
    #[test]
    fn descriptor_suffix_classifies_each_segment_kind() {
        assert_eq!(
            kind_from_descriptor_suffix("foo/bar/"),
            Some(Kind::Namespace)
        );
        assert_eq!(kind_from_descriptor_suffix("Foo#"), Some(Kind::Class));
        assert_eq!(kind_from_descriptor_suffix("Foo#bar."), Some(Kind::Field));
        assert_eq!(
            kind_from_descriptor_suffix("Foo#bar()."),
            Some(Kind::Method)
        );
        assert_eq!(kind_from_descriptor_suffix("foo!"), Some(Kind::Macro));
        assert_eq!(kind_from_descriptor_suffix("foo:"), Some(Kind::Constant));
        assert_eq!(
            kind_from_descriptor_suffix("foo().[T]"),
            Some(Kind::TypeParameter)
        );
        assert_eq!(
            kind_from_descriptor_suffix("foo().(x)"),
            Some(Kind::Parameter)
        );
        assert!(kind_from_descriptor_suffix("").is_none());
    }

    #[test]
    fn unspecified_indexer_kind_falls_through_to_variable() {
        assert_eq!(
            kind_from_scip_go_kind(ScipKind::UnspecifiedKind),
            Kind::Variable
        );
    }

    /// `ScipKind::from_i32` is the wire-protocol decoder for SCIP's
    /// `Symbol.kind` field. Cover every discriminant the indexers can
    /// observe (the cases in the `match`) plus the catch-all `None`
    /// arm for unknown values.
    #[test]
    fn scip_kind_from_i32_covers_every_discriminant() {
        for (n, expected) in [
            (0, ScipKind::UnspecifiedKind),
            (7, ScipKind::Class),
            (8, ScipKind::Constant),
            (9, ScipKind::Constructor),
            (11, ScipKind::Enum),
            (12, ScipKind::EnumMember),
            (13, ScipKind::Event),
            (15, ScipKind::Field),
            (17, ScipKind::Function),
            (21, ScipKind::Interface),
            (25, ScipKind::Macro),
            (26, ScipKind::Method),
            (29, ScipKind::Module),
            (30, ScipKind::Namespace),
            (34, ScipKind::Operator),
            (35, ScipKind::Package),
            (37, ScipKind::Parameter),
            (41, ScipKind::Property),
            (49, ScipKind::Struct),
            (53, ScipKind::Trait),
            (55, ScipKind::Type),
            (56, ScipKind::TypeAlias),
            (60, ScipKind::TypeParameter),
            (63, ScipKind::Variable),
            (66, ScipKind::AbstractMethod),
            (70, ScipKind::StaticMethod),
            (72, ScipKind::Accessor),
        ] {
            assert_eq!(ScipKind::from_i32(n), Some(expected), "discriminant {n}");
        }
        // Unknown values map to None (catch-all arm).
        for n in [-1, 1, 2, 3, 4, 5, 6, 10, 100, 9999] {
            assert!(
                ScipKind::from_i32(n).is_none(),
                "discriminant {n} should be None"
            );
        }
    }
}
