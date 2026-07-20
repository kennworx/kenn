use serde::{Deserialize, Serialize};

/// Closed kind enum (design D7). Mapped from indexer-emitted kinds or
/// derived from SCIP descriptor suffixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Package,
    Module,
    Namespace,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    EnumMember,
    TypeAlias,
    Method,
    Function,
    Constructor,
    Destructor,
    Operator,
    Field,
    Property,
    Constant,
    Variable,
    Parameter,
    TypeParameter,
    Macro,
    /// A markdown file as a navigable node (link target for the whole file).
    Document,
    /// A markdown heading and the section it owns.
    Section,
    /// A non-markdown asset referenced by a wikilink/embed (e.g.
    /// `![[diagram.png]]`). kenn does not index binary assets, so an
    /// attachment is a leaf stub node, not a navigable document. Its
    /// concrete type is the file extension's MIME type.
    Attachment,
    /// A CSS/Sass class selector (`.btn`) as a node. Shared across the `css`
    /// and `sass` languages — a class is a class regardless of source. The
    /// stylesheet file itself reuses [`Self::Module`].
    CssClass,
    /// A CSS/Sass id selector (`#app`) as a node.
    CssId,
    /// A CSS custom property (`--brand`) definition as a node.
    CssVar,
    /// An HTML element `id="…"` as a node, owned by the HTML document. HTML
    /// owns id definitions; a same-named CSS `#id` selector ([`Self::CssId`])
    /// joins it via `corresponds_to`, not a usage edge.
    HtmlId,
    /// One size-bounded fragment of a text-fallback file (yaml/json/txt/…): the
    /// searchable unit the generic recursive splitter emits, owned by its file
    /// [`Self::Document`]. It has no heading (unlike [`Self::Section`]); its
    /// chunk text is the embeddable/FTS prose.
    Chunk,
}

impl Kind {
    /// Every variant, in declaration order — the single enumeration that
    /// [`Self::from_db_name`] and the round-trip / uniqueness tests iterate, so
    /// a new kind is covered by adding it here once.
    pub const ALL: [Self; 30] = [
        Self::Package,
        Self::Module,
        Self::Namespace,
        Self::Class,
        Self::Struct,
        Self::Interface,
        Self::Trait,
        Self::Enum,
        Self::EnumMember,
        Self::TypeAlias,
        Self::Method,
        Self::Function,
        Self::Constructor,
        Self::Destructor,
        Self::Operator,
        Self::Field,
        Self::Property,
        Self::Constant,
        Self::Variable,
        Self::Parameter,
        Self::TypeParameter,
        Self::Macro,
        Self::Document,
        Self::Section,
        Self::Attachment,
        Self::CssClass,
        Self::CssId,
        Self::CssVar,
        Self::HtmlId,
        Self::Chunk,
    ];

    /// Lowercase `snake_case` name persisted in `symbols.kind`.
    #[must_use]
    pub const fn db_name(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Module => "module",
            Self::Namespace => "namespace",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Interface => "interface",
            Self::Trait => "trait",
            Self::Enum => "enum",
            Self::EnumMember => "enum_member",
            Self::TypeAlias => "type_alias",
            Self::Method => "method",
            Self::Function => "function",
            Self::Constructor => "constructor",
            Self::Destructor => "destructor",
            Self::Operator => "operator",
            Self::Field => "field",
            Self::Property => "property",
            Self::Constant => "constant",
            Self::Variable => "variable",
            Self::Parameter => "parameter",
            Self::TypeParameter => "type_parameter",
            Self::Macro => "macro",
            Self::Document => "document",
            Self::Section => "section",
            Self::Attachment => "attachment",
            Self::CssClass => "css_class",
            Self::CssId => "css_id",
            Self::CssVar => "css_var",
            Self::HtmlId => "html_id",
            Self::Chunk => "chunk",
        }
    }

    /// Parse the lowercase `snake_case` form produced by [`Self::db_name`]
    /// back into a `Kind`. Returns `None` for unknown strings. Table-driven over
    /// [`Self::ALL`] so the inverse stays a single mapping, not a parallel match.
    #[must_use]
    pub fn from_db_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.db_name() == s)
    }

    /// True if this kind groups other symbols (target of `defined_in`,
    /// source of `contains` / `imports`).
    #[must_use]
    pub const fn is_scope(self) -> bool {
        matches!(self, Self::Package | Self::Module | Self::Namespace)
    }

    /// True for nominal type kinds — classes, structs, traits,
    /// interfaces, enums, type aliases. Used by aggregate-id rollup
    /// (nearest enclosing class-like) and by the Python test-descriptor
    /// heuristic (unittest `Test*` / `*TestCase` class shape).
    #[must_use]
    pub const fn is_class_like(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Struct
                | Self::Trait
                | Self::Interface
                | Self::Enum
                | Self::TypeAlias
        )
    }

    /// True if `args_arity` is meaningful for this kind.
    #[must_use]
    pub const fn is_callable(self) -> bool {
        matches!(
            self,
            Self::Method | Self::Function | Self::Constructor | Self::Destructor | Self::Operator
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn db_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in Kind::ALL {
            assert!(
                seen.insert(k.db_name()),
                "duplicate db_name: {}",
                k.db_name()
            );
        }
        assert_eq!(seen.len(), Kind::ALL.len());
    }

    /// Round-trip `db_name` through `from_db_name` for every variant,
    /// plus the unknown-string `None` arm.
    #[test]
    fn from_db_name_round_trips_every_variant() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_db_name(k.db_name()), Some(k), "round-trip {k:?}");
        }
        for unknown in ["", "Class", "klass", "package_", " package"] {
            assert!(
                Kind::from_db_name(unknown).is_none(),
                "{unknown:?} must not parse"
            );
        }
    }
}
