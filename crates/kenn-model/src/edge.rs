use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::record::ShortId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Symbol → enclosing module/namespace/package (D8).
    DefinedIn,
    /// Module/namespace/package → file (D9, M:N).
    Contains,
    /// Caller → callee (D10).
    Calls,
    /// User → referenced type (D10).
    TypeUse,
    /// Reader/writer → field (D10).
    FieldAccess,
    /// Concrete type → interface/trait (D10).
    Implements,
    /// Override → base method (D10).
    Overrides,
    /// Generic type → type argument (D10).
    Instantiates,
    /// Type parameter → constraint type (D10).
    GenericConstraint,
    /// Module → module (D12).
    Imports,
    /// Symbol ↔ equivalent symbol (D11).
    CorrespondsTo,
    /// Augmenting symbol → the type it extends from outside the type's own
    /// declaration (e.g. a C# extension method → its receiver type). A
    /// non-containment edge: the source keeps its `DefinedIn` to its real
    /// declaring scope. Parallels `ExtendsRule` for stylesheets.
    ExtendsType,
    /// Markdown reference: linking node → linked node.
    LinksTo,
    /// Markdown transclusion: host node inlines the target's content.
    Embeds,
    /// Markdown reference whose target is a code FILE node. A distinct kind (not
    /// `LinksTo`) so the file-vs-symbol target table is disambiguated by edge
    /// kind — the same trick `Contains` uses — since file and symbol ids collide
    /// in the `ShortId` space. Hydrated from the files table, never `symbols`.
    LinksToFile,
    /// A code file/symbol references a CSS class (`className="btn"`). Source is
    /// the enclosing code symbol or file; target is a `css_class` node. Emitted
    /// only on a registry hit (no dangling stubs).
    UsesCssClass,
    /// A CSS rule extends another (`@extend .base`, CSS-Modules `composes`).
    /// Source is the extending `css_class` node; target is the extended
    /// `css_class`. Emitted only on a registry hit (no dangling stubs).
    ExtendsRule,
    /// A statement or element brings a table into being (`CREATE TABLE`).
    /// Marks the table internal — it does NOT gate the table's existence, since
    /// any reference may mint one. Source is the reference site, target the
    /// `sql_table` node.
    DefinesTable,
    /// A statement or element changes an existing table's definition
    /// (`ALTER TABLE`, `DROP TABLE`). A modification never mints identity: an
    /// alter of an undeclared table links to a table minted by the reference
    /// itself, and the index does not evaluate history, so a drop does not
    /// unregister its target.
    AltersTable,
    /// A statement or element reads or writes a table's data. Source is the
    /// reference site, target the `sql_table` node.
    AccessesTable,
}

impl EdgeKind {
    /// Lowercase name used both in serialized records and as the DB relation
    /// table name.
    #[must_use]
    pub const fn db_name(self) -> &'static str {
        match self {
            Self::DefinedIn => "defined_in",
            Self::Contains => "contains",
            Self::Calls => "calls",
            Self::TypeUse => "type_use",
            Self::FieldAccess => "field_access",
            Self::Implements => "implements",
            Self::Overrides => "overrides",
            Self::Instantiates => "instantiates",
            Self::GenericConstraint => "generic_constraint",
            Self::Imports => "imports",
            Self::CorrespondsTo => "corresponds_to",
            Self::ExtendsType => "extends_type",
            Self::LinksTo => "links_to",
            Self::Embeds => "embeds",
            Self::LinksToFile => "links_to_file",
            Self::UsesCssClass => "uses_css_class",
            Self::ExtendsRule => "extends_rule",
            Self::DefinesTable => "defines_table",
            Self::AltersTable => "alters_table",
            Self::AccessesTable => "accesses_table",
        }
    }
}

/// A stored edge-kind `u32` that maps to no known [`EdgeKind`] — a variant added
/// by a newer binary and read by an older one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownEdgeKindCode(pub u32);

impl From<EdgeKind> for NonZeroU32 {
    /// The stable on-disk discriminant, **1-based** — code `0` is reserved as a
    /// null/none sentinel, so a valid edge kind is always `NonZeroU32`. NOTE: not
    /// declaration order — `ExtendsType` is appended (17) so adding a variant never
    /// shifts an existing code. Keep in lockstep with the `TryFrom` inverse below.
    fn from(kind: EdgeKind) -> Self {
        let code: u32 = match kind {
            EdgeKind::DefinedIn => 1,
            EdgeKind::Contains => 2,
            EdgeKind::Calls => 3,
            EdgeKind::TypeUse => 4,
            EdgeKind::FieldAccess => 5,
            EdgeKind::Implements => 6,
            EdgeKind::Overrides => 7,
            EdgeKind::Instantiates => 8,
            EdgeKind::GenericConstraint => 9,
            EdgeKind::Imports => 10,
            EdgeKind::CorrespondsTo => 11,
            EdgeKind::LinksTo => 12,
            EdgeKind::Embeds => 13,
            EdgeKind::LinksToFile => 14,
            EdgeKind::UsesCssClass => 15,
            EdgeKind::ExtendsRule => 16,
            EdgeKind::ExtendsType => 17,
            EdgeKind::DefinesTable => 18,
            EdgeKind::AltersTable => 19,
            EdgeKind::AccessesTable => 20,
        };
        NonZeroU32::new(code).expect("edge-kind codes are 1..=20, never 0")
    }
}

impl TryFrom<u32> for EdgeKind {
    type Error = UnknownEdgeKindCode;
    /// O(1) inverse of `NonZeroU32::from`. Code `0` (the null sentinel) and any
    /// unknown code are errors, not a silent fallback — callers on the hot read
    /// path choose their own policy.
    fn try_from(code: u32) -> Result<Self, Self::Error> {
        Ok(match code {
            1 => Self::DefinedIn,
            2 => Self::Contains,
            3 => Self::Calls,
            4 => Self::TypeUse,
            5 => Self::FieldAccess,
            6 => Self::Implements,
            7 => Self::Overrides,
            8 => Self::Instantiates,
            9 => Self::GenericConstraint,
            10 => Self::Imports,
            11 => Self::CorrespondsTo,
            12 => Self::LinksTo,
            13 => Self::Embeds,
            14 => Self::LinksToFile,
            15 => Self::UsesCssClass,
            16 => Self::ExtendsRule,
            17 => Self::ExtendsType,
            18 => Self::DefinesTable,
            19 => Self::AltersTable,
            20 => Self::AccessesTable,
            other => return Err(UnknownEdgeKindCode(other)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FieldOp {
    Read,
    Write,
}

/// Resolution-quality grade carried by a markdown `LinksTo`/`Embeds` edge.
/// The resolution ladder downgrades through these rather than failing: a
/// stale path/qualifier resolves as `Drifted`, an approximate name as
/// `Fuzzy`, an unresolved name as `Dangling` (an edge to an external stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinkGrade {
    /// Path/qualifier and name both current.
    Exact,
    /// Name current, path or qualifier stale.
    Drifted,
    /// Name approximate (case/typo/partial).
    Fuzzy,
    /// Multiple name matches; this is one of several kept candidates.
    Ambiguous,
    /// No name match; edge points at an unresolved external stub.
    Dangling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    Explicit,
    ReExport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsomorphismSource {
    Config,
    AutoInferred,
    Codegen,
}

/// Per-edge-kind property bag. Empty variants for kinds that carry no
/// properties; concrete variants for kinds that do.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "edge_kind", rename_all = "snake_case")]
pub enum EdgeProperties {
    DefinedIn,
    Contains,
    Calls,
    TypeUse,
    FieldAccess {
        op: FieldOp,
    },
    Implements,
    Overrides,
    Instantiates,
    GenericConstraint,
    Imports {
        kind: ImportKind,
    },
    CorrespondsTo {
        source: IsomorphismSource,
        #[serde(default)]
        generator: String,
        #[serde(default)]
        canonical: ShortId,
    },
    ExtendsType,
    LinksTo {
        grade: LinkGrade,
        /// Typed-frontmatter relation (e.g. `supports`, `extends`); empty
        /// for a plain structural link.
        #[serde(default)]
        relation: String,
    },
    Embeds {
        grade: LinkGrade,
    },
    LinksToFile {
        grade: LinkGrade,
    },
    UsesCssClass {
        grade: LinkGrade,
    },
    ExtendsRule {
        grade: LinkGrade,
    },
    /// A reference site names a table. One shape covers all three table edge
    /// kinds because they differ only in what the site does to the table, and a
    /// single site can carry more than one — a create-as-select both defines its
    /// target and accesses its sources.
    Table {
        kind: EdgeKind,
        grade: LinkGrade,
    },
}

impl EdgeProperties {
    #[must_use]
    pub const fn kind(&self) -> EdgeKind {
        match self {
            Self::DefinedIn => EdgeKind::DefinedIn,
            Self::Contains => EdgeKind::Contains,
            Self::Calls => EdgeKind::Calls,
            Self::TypeUse => EdgeKind::TypeUse,
            Self::FieldAccess { .. } => EdgeKind::FieldAccess,
            Self::Implements => EdgeKind::Implements,
            Self::Overrides => EdgeKind::Overrides,
            Self::Instantiates => EdgeKind::Instantiates,
            Self::GenericConstraint => EdgeKind::GenericConstraint,
            Self::Imports { .. } => EdgeKind::Imports,
            Self::CorrespondsTo { .. } => EdgeKind::CorrespondsTo,
            Self::ExtendsType => EdgeKind::ExtendsType,
            Self::LinksTo { .. } => EdgeKind::LinksTo,
            Self::Embeds { .. } => EdgeKind::Embeds,
            Self::LinksToFile { .. } => EdgeKind::LinksToFile,
            Self::UsesCssClass { .. } => EdgeKind::UsesCssClass,
            Self::ExtendsRule { .. } => EdgeKind::ExtendsRule,
            Self::Table { kind, .. } => *kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeRecord {
    pub src_id: ShortId,
    pub target_id: ShortId,
    pub properties: EdgeProperties,
}

impl EdgeRecord {
    #[must_use]
    pub fn kind(&self) -> EdgeKind {
        self.properties.kind()
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeKind, EdgeProperties, FieldOp, ImportKind, IsomorphismSource, LinkGrade};

    /// 2.8 — every relation listed in design D8/D9/D10/D11/D12 has an
    /// `EdgeKind` variant. This is the static counterpart of the design table.
    #[test]
    fn edge_kind_covers_design_d8_through_d12() {
        let expected = [
            "defined_in",         // D8
            "contains",           // D9
            "calls",              // D10
            "type_use",           // D10
            "field_access",       // D10
            "implements",         // D10
            "overrides",          // D10
            "instantiates",       // D10
            "generic_constraint", // D10
            "corresponds_to",     // D11
            "imports",            // D12
        ];
        let actual: std::collections::HashSet<&str> = [
            EdgeKind::DefinedIn,
            EdgeKind::Contains,
            EdgeKind::Calls,
            EdgeKind::TypeUse,
            EdgeKind::FieldAccess,
            EdgeKind::Implements,
            EdgeKind::Overrides,
            EdgeKind::Instantiates,
            EdgeKind::GenericConstraint,
            EdgeKind::CorrespondsTo,
            EdgeKind::Imports,
        ]
        .iter()
        .map(|k| k.db_name())
        .collect();
        for name in expected {
            assert!(actual.contains(name), "missing edge kind: {name}");
        }
        assert_eq!(actual.len(), expected.len());
    }

    /// `EdgeProperties::kind` is the mapping from the rich-variant
    /// `EdgeProperties` (which can carry per-edge data like `FieldOp`
    /// or `ImportKind`) down to its discriminant `EdgeKind`. Every
    /// variant must map to its kin.
    #[test]
    fn edge_properties_kind_covers_every_variant() {
        for (props, expected) in [
            (EdgeProperties::DefinedIn, EdgeKind::DefinedIn),
            (EdgeProperties::Contains, EdgeKind::Contains),
            (EdgeProperties::Calls, EdgeKind::Calls),
            (EdgeProperties::TypeUse, EdgeKind::TypeUse),
            (
                EdgeProperties::FieldAccess { op: FieldOp::Read },
                EdgeKind::FieldAccess,
            ),
            (
                EdgeProperties::FieldAccess { op: FieldOp::Write },
                EdgeKind::FieldAccess,
            ),
            (EdgeProperties::Implements, EdgeKind::Implements),
            (EdgeProperties::Overrides, EdgeKind::Overrides),
            (EdgeProperties::Instantiates, EdgeKind::Instantiates),
            (
                EdgeProperties::GenericConstraint,
                EdgeKind::GenericConstraint,
            ),
            (
                EdgeProperties::Imports {
                    kind: ImportKind::Explicit,
                },
                EdgeKind::Imports,
            ),
            (
                EdgeProperties::CorrespondsTo {
                    source: IsomorphismSource::Codegen,
                    generator: "openapi".into(),
                    canonical: 0,
                },
                EdgeKind::CorrespondsTo,
            ),
            (EdgeProperties::ExtendsType, EdgeKind::ExtendsType),
            (
                EdgeProperties::LinksTo {
                    grade: LinkGrade::Drifted,
                    relation: String::new(),
                },
                EdgeKind::LinksTo,
            ),
            (
                EdgeProperties::Embeds {
                    grade: LinkGrade::Exact,
                },
                EdgeKind::Embeds,
            ),
            (
                EdgeProperties::LinksToFile {
                    grade: LinkGrade::Drifted,
                },
                EdgeKind::LinksToFile,
            ),
            (
                EdgeProperties::UsesCssClass {
                    grade: LinkGrade::Exact,
                },
                EdgeKind::UsesCssClass,
            ),
            (
                EdgeProperties::ExtendsRule {
                    grade: LinkGrade::Exact,
                },
                EdgeKind::ExtendsRule,
            ),
        ] {
            assert_eq!(props.kind(), expected);
        }
    }
}
