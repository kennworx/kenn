use serde::{Deserialize, Serialize};

/// Closed kind enum (design D7). Mapped from indexer-emitted kinds or
/// derived from SCIP descriptor suffixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::IntoStaticStr)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
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
    /// A database table as a node. Workspace-global rather than file-scoped: a
    /// table has no enclosing symbol, so it is its own aggregate. Prefixed
    /// because markdown and HTML tables are plausible future kinds.
    SqlTable,
    /// One top-level SQL statement, owned by its file. A single kind covers
    /// every statement shape — a create-as-select both defines and accesses, so
    /// the role belongs on the edge, where it can be plural.
    SqlStatement,
    /// One element of an XML document, owned by its enclosing element and
    /// ultimately its [`Self::Document`]. Attributes and text ride on it rather
    /// than becoming nodes, which is what bounds the graph to the element count.
    XmlElement,
}

impl Kind {
    /// Every variant, in declaration order — the single enumeration that
    /// [`Self::from_db_name`] and the round-trip / uniqueness tests iterate, so
    /// a new kind is covered by adding it here once.
    pub const ALL: [Self; 33] = [
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
        Self::SqlTable,
        Self::SqlStatement,
        Self::XmlElement,
    ];

    /// Lowercase `snake_case` name persisted in `symbols.kind`.
    ///
    /// Derived rather than hand-written. A 33-arm `match` is exhaustive — the
    /// compiler rejects a variant with no name — but it is also 33 branches of
    /// measured complexity for a table with no logic in it, and CRAP collapses
    /// to the branch count once a function is fully covered. The derive expands
    /// over every variant, so a new kind still cannot silently miss its name,
    /// and the mapping stops being something to maintain.
    ///
    /// Not `const fn`: the derive produces a `From` impl, which cannot be const.
    /// No caller needs it in a const context.
    #[must_use]
    pub fn db_name(self) -> &'static str {
        self.into()
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

    /// The literal on-disk vocabulary, pinned.
    ///
    /// Neither test above can catch a wrong name. `from_db_name` is
    /// `ALL.find(|k| k.db_name() == s)`, so the round-trip compares `db_name`
    /// against itself and passes for ANY naming — and uniqueness only asks that
    /// the names differ. If the derive rendered `TypeAlias` as `type-alias`,
    /// both would stay green while every persisted `symbols.kind` value silently
    /// changed meaning and every existing snapshot became unreadable.
    ///
    /// These strings are a storage format. They are pinned here so a rename is
    /// a deliberate, visible edit rather than a side effect of a derive
    /// attribute.
    #[test]
    fn db_names_are_the_pinned_on_disk_vocabulary() {
        const PINNED: [(Kind, &str); 33] = [
            (Kind::Package, "package"),
            (Kind::Module, "module"),
            (Kind::Namespace, "namespace"),
            (Kind::Class, "class"),
            (Kind::Struct, "struct"),
            (Kind::Interface, "interface"),
            (Kind::Trait, "trait"),
            (Kind::Enum, "enum"),
            (Kind::EnumMember, "enum_member"),
            (Kind::TypeAlias, "type_alias"),
            (Kind::Method, "method"),
            (Kind::Function, "function"),
            (Kind::Constructor, "constructor"),
            (Kind::Destructor, "destructor"),
            (Kind::Operator, "operator"),
            (Kind::Field, "field"),
            (Kind::Property, "property"),
            (Kind::Constant, "constant"),
            (Kind::Variable, "variable"),
            (Kind::Parameter, "parameter"),
            (Kind::TypeParameter, "type_parameter"),
            (Kind::Macro, "macro"),
            (Kind::Document, "document"),
            (Kind::Section, "section"),
            (Kind::Attachment, "attachment"),
            (Kind::CssClass, "css_class"),
            (Kind::CssId, "css_id"),
            (Kind::CssVar, "css_var"),
            (Kind::HtmlId, "html_id"),
            (Kind::Chunk, "chunk"),
            (Kind::SqlTable, "sql_table"),
            (Kind::SqlStatement, "sql_statement"),
            (Kind::XmlElement, "xml_element"),
        ];
        assert_eq!(
            PINNED.len(),
            Kind::ALL.len(),
            "a new kind was added without pinning its on-disk name"
        );
        for (kind, expected) in PINNED {
            assert_eq!(kind.db_name(), expected, "on-disk name for {kind:?}");
        }
    }
}
