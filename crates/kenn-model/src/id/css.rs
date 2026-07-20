//! CSS/Sass public-ID construction.
//!
//! Stylesheet nodes are not produced from SCIP; their native IDs are built from
//! the file's workspace-relative path plus a typed selector fragment. The public
//! ID is `<lang>:<relpath>` for the stylesheet-as-module node and
//! `<lang>:<relpath>#<type>:<name>` for a selector node, where `<type>` is one
//! of `class` / `id` / `var`. The `<type>` segment keeps a class and an id of
//! the same name in one file distinct (`.hero` vs `#hero`). `<lang>` is `css`
//! for `.css` and `sass` for `.scss`/`.sass`.

use crate::id::PublicId;
use crate::language::Language;

/// The selector type segment of a stylesheet node's native ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    Class,
    Id,
    Var,
}

impl SelectorKind {
    const fn tag(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Id => "id",
            Self::Var => "var",
        }
    }
}

/// Public ID of a stylesheet file-as-node (`module` kind): `<lang>:<relpath>`.
#[must_use]
pub fn module_id(language: Language, relpath: &str) -> PublicId {
    PublicId::new(language, relpath)
}

/// Public ID of a selector node: `<lang>:<relpath>#<type>:<name>`. `name` is the
/// bare token for a class (`btn`) or id (`app`), and the full custom-property
/// name including `--` for a var (`--brand`).
#[must_use]
pub fn selector_id(language: Language, relpath: &str, kind: SelectorKind, name: &str) -> PublicId {
    PublicId::new(language, &format!("{relpath}#{}:{name}", kind.tag()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_selector_ids() {
        assert_eq!(
            selector_id(
                Language::Css,
                "src/button.css",
                SelectorKind::Class,
                "btn-primary"
            )
            .as_str(),
            "css:src/button.css#class:btn-primary"
        );
        assert_eq!(
            selector_id(Language::Css, "src/app.css", SelectorKind::Id, "app").as_str(),
            "css:src/app.css#id:app"
        );
        assert_eq!(
            selector_id(Language::Css, "src/theme.css", SelectorKind::Var, "--brand").as_str(),
            "css:src/theme.css#var:--brand"
        );
    }

    #[test]
    fn class_and_id_same_name_are_distinct() {
        let class = selector_id(Language::Css, "x.css", SelectorKind::Class, "hero");
        let id = selector_id(Language::Css, "x.css", SelectorKind::Id, "hero");
        assert_ne!(class.as_str(), id.as_str());
        assert_eq!(class.as_str(), "css:x.css#class:hero");
        assert_eq!(id.as_str(), "css:x.css#id:hero");
    }

    #[test]
    fn css_and_sass_prefixes_differ() {
        assert_eq!(
            selector_id(Language::Css, "a.css", SelectorKind::Class, "btn").as_str(),
            "css:a.css#class:btn"
        );
        assert_eq!(
            selector_id(Language::Sass, "a.scss", SelectorKind::Class, "btn").as_str(),
            "sass:a.scss#class:btn"
        );
    }

    #[test]
    fn module_id_is_the_bare_path() {
        assert_eq!(
            module_id(Language::Css, "src/button.css").as_str(),
            "css:src/button.css"
        );
        assert_eq!(
            module_id(Language::Sass, "src/button.scss").as_str(),
            "sass:src/button.scss"
        );
    }
}
