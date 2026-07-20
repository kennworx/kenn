//! HTML public-ID construction.
//!
//! HTML nodes are not produced from SCIP; their native IDs are built from the
//! file's workspace-relative path. The public ID is `html:<relpath>` for the
//! HTML file-as-`document` node and `html:<relpath>#id:<name>` for an `html_id`
//! node, where `<name>` is the bare `id="…"` value. The `#id:` type segment
//! mirrors the stylesheet scheme (`<lang>:<relpath>#<type>:<name>`), so an
//! HTML id and a CSS class/id of the same name in the same file never collide.

use crate::id::PublicId;
use crate::language::Language;

/// Public ID of an HTML file-as-node (`document` kind): `html:<relpath>`.
#[must_use]
pub fn document_id(relpath: &str) -> PublicId {
    PublicId::new(Language::Html, relpath)
}

/// Public ID of an `html_id` node: `html:<relpath>#id:<name>`. `name` is the
/// bare `id="…"` value.
#[must_use]
pub fn html_id(relpath: &str, name: &str) -> PublicId {
    PublicId::new(Language::Html, &format!("{relpath}#id:{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::css::{selector_id, SelectorKind};

    #[test]
    fn document_id_is_the_bare_path() {
        assert_eq!(
            document_id("pages/index.html").as_str(),
            "html:pages/index.html"
        );
    }

    #[test]
    fn typed_html_id() {
        assert_eq!(
            html_id("pages/index.html", "root").as_str(),
            "html:pages/index.html#id:root"
        );
    }

    #[test]
    fn two_ids_in_one_file_are_distinct() {
        let a = html_id("page.html", "root");
        let b = html_id("page.html", "header");
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn html_id_and_same_named_class_never_collide() {
        // The document owns both namespaces under the `html:` prefix; an id and
        // an inline-style class of the same name (design D6) stay distinct via
        // the `#id:` vs `#class:` type segment.
        let id = html_id("page.html", "hero");
        let class = selector_id(Language::Html, "page.html", SelectorKind::Class, "hero");
        assert_ne!(id.as_str(), class.as_str());
        assert_eq!(id.as_str(), "html:page.html#id:hero");
        assert_eq!(class.as_str(), "html:page.html#class:hero");
    }
}
