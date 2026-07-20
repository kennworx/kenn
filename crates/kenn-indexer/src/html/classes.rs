//! HTML `class=` usage attribution (design D1/D5, Phase 4 task 6.1).
//!
//! The HTML parser owns class-usage for HTML files (design D5): each real
//! `class="…"` attribute token is intersected with the shared CSS class registry
//! and, on a hit, emits a `uses_css_class` edge. The edge source is the **nearest
//! enclosing id'd element's `html_id` node** — the same "enclosing symbol, else
//! file" attribution `index-css` uses, where the document node is the file
//! fallback (design D2). Because [`parse_elements`](super::parse) already scopes
//! attributes to real elements (comment / `<script>` text never reaches an
//! `Element`), the parser path emits none of the phantom edges the raw
//! `usage_sources` scan would — which is why that scan excludes indexed-HTML
//! extensions (task 6.1a).
//!
//! Pure over a [`parse_elements`](super::parse) element list plus a caller-built
//! [`ClassRegistry`] and the file's [`HtmlIdIndex`]; the store-backed registry
//! and pipeline wiring live in the barrier pass (task 6.2).

use std::collections::HashSet;

use kenn_model::{EdgeProperties, EdgeRecord, LinkGrade, ShortId};

use super::ids::HtmlIdIndex;
use super::parse::{Attr, Element};

/// Lookup over the shared CSS class registry — a bare class name → the
/// `css_class` node ids that define it (empty = not a defined class). The same
/// registry the css usage scan resolves against; implemented against the
/// building store post-barrier, mocked in tests.
pub trait ClassRegistry {
    /// The `css_class` node ids whose bare name equals `name`.
    fn class_ids(&self, name: &str) -> Vec<ShortId>;
}

/// `uses_css_class` edges for one file's real `class="…"` attributes (task 6.1).
/// Each whitespace-separated token is intersected with `registry`; a hit emits an
/// edge from the nearest enclosing `html_id` node (walking the element `parent`
/// chain against this file's `id_index`), else from the document node
/// (`doc_sym`). A token with no registry entry emits nothing — no edge, no node.
/// Grades mirror the css usage scan: `Exact` for the single-def class attribute,
/// `Ambiguous` when the name has several definitions. Pure over its inputs;
/// deduped on `(src, target)`.
#[must_use]
pub fn class_usage_edges(
    elements: &[Element],
    doc_sym: ShortId,
    id_index: &HtmlIdIndex,
    registry: &dyn ClassRegistry,
) -> Vec<EdgeRecord> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(ShortId, ShortId)> = HashSet::new();
    for (idx, el) in elements.iter().enumerate() {
        let Some(class) = class_attr(el) else {
            continue;
        };
        let src = enclosing_id(elements, idx, id_index, doc_sym);
        for token in class.split_whitespace() {
            let ids = registry.class_ids(token);
            let grade = if ids.len() > 1 {
                LinkGrade::Ambiguous
            } else {
                LinkGrade::Exact
            };
            for class_id in ids {
                if seen.insert((src, class_id)) {
                    edges.push(EdgeRecord {
                        src_id: src,
                        target_id: class_id,
                        properties: EdgeProperties::UsesCssClass { grade },
                    });
                }
            }
        }
    }
    edges
}

/// The first `class` attribute value of an element (ASCII-folded name), if present.
fn class_attr(el: &Element) -> Option<&str> {
    el.attrs
        .iter()
        .find(|a: &&Attr| a.name == "class")
        .map(|a| a.value.as_str())
}

/// The `html_id` node nearest-enclosing the element at `idx`: walk the element
/// itself then its `parent` chain, returning the first ancestor whose `id`
/// attribute names a node in `id_index`; falls back to `doc_sym` (the document
/// node — the file-level attribution, design D2).
fn enclosing_id(
    elements: &[Element],
    idx: usize,
    id_index: &HtmlIdIndex,
    doc_sym: ShortId,
) -> ShortId {
    let mut cursor = Some(idx);
    while let Some(el) = cursor.and_then(|i| elements.get(i)) {
        if let Some(id) = el.attrs.iter().find(|a| a.name == "id") {
            if let Some(node) = id_index.get(id.value.trim()) {
                return node;
            }
        }
        cursor = el.parent;
    }
    doc_sym
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::ids::html_id_nodes;
    use crate::html::links::HtmlIds;
    use crate::html::parse::parse_elements;
    use kenn_model::{compose_short_id, Language};
    use std::collections::HashMap;

    fn doc() -> ShortId {
        compose_short_id(Language::Html, 1)
    }
    fn file() -> ShortId {
        compose_short_id(Language::Html, 2)
    }

    /// A name → class-node-id registry for tests.
    struct Registry(HashMap<String, Vec<ShortId>>);
    impl ClassRegistry for Registry {
        fn class_ids(&self, name: &str) -> Vec<ShortId> {
            self.0.get(name).cloned().unwrap_or_default()
        }
    }
    fn registry(pairs: &[(&str, &[u32])]) -> Registry {
        Registry(
            pairs
                .iter()
                .map(|(n, ids)| {
                    (
                        (*n).to_string(),
                        ids.iter()
                            .map(|k| compose_short_id(Language::Css, *k))
                            .collect(),
                    )
                })
                .collect(),
        )
    }

    /// Run the `html_id` pass to build the file's id index, then attribute usage.
    fn usage(html: &str, reg: &Registry) -> Vec<EdgeRecord> {
        let els = parse_elements(html);
        let mut ids = HtmlIds::new(1000);
        let nodes = html_id_nodes(&els, "page.html", doc(), file(), &mut ids);
        class_usage_edges(&els, doc(), &nodes.index, reg)
    }

    #[test]
    fn class_attribute_becomes_usage_edge_from_document() {
        let edges = usage(r#"<span class="btn">x</span>"#, &registry(&[("btn", &[7])]));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, doc());
        assert_eq!(edges[0].target_id, compose_short_id(Language::Css, 7));
        assert_eq!(
            edges[0].properties,
            EdgeProperties::UsesCssClass {
                grade: LinkGrade::Exact
            }
        );
    }

    #[test]
    fn usage_attributes_to_enclosing_id_element() {
        // <div id="card"><span class="btn"> → the edge source is the `card`
        // html_id node, not the document node.
        let els = parse_elements(r#"<div id="card"><span class="btn">x</span></div>"#);
        let mut ids = HtmlIds::new(1000);
        let nodes = html_id_nodes(&els, "page.html", doc(), file(), &mut ids);
        let card = nodes.index.get("card").expect("card html_id");
        let edges = class_usage_edges(&els, doc(), &nodes.index, &registry(&[("btn", &[7])]));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, card);
        assert_eq!(edges[0].target_id, compose_short_id(Language::Css, 7));
    }

    #[test]
    fn unregistered_token_emits_nothing() {
        let edges = usage(
            r#"<span class="btn ghost">x</span>"#,
            &registry(&[("btn", &[7])]),
        );
        // `ghost` is not in the registry → only `btn` produces an edge.
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, compose_short_id(Language::Css, 7));
    }

    #[test]
    fn comment_class_produces_no_usage() {
        // `<!-- class="ghost" -->` never reaches an Element, so no edge — even if
        // `ghost` were registered (spec scenario).
        let edges = usage(
            r#"<!-- <div class="ghost"> --><span class="btn">x</span>"#,
            &registry(&[("btn", &[7]), ("ghost", &[8])]),
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, compose_short_id(Language::Css, 7));
    }

    #[test]
    fn multiple_definitions_grade_ambiguous() {
        let edges = usage(
            r#"<span class="btn">x</span>"#,
            &registry(&[("btn", &[7, 8])]),
        );
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|e| e.properties
            == EdgeProperties::UsesCssClass {
                grade: LinkGrade::Ambiguous
            }));
    }

    #[test]
    fn repeated_class_on_same_source_deduped() {
        // Two `class="btn"` under the same document fallback → one edge.
        let edges = usage(
            r#"<span class="btn">a</span><span class="btn">b</span>"#,
            &registry(&[("btn", &[7])]),
        );
        assert_eq!(edges.len(), 1);
    }
}
