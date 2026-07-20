//! HTML `html_id` nodes and CSS-id correspondence (design D1 Tier 2, Phase 2).
//!
//! HTML *owns* the id (design D2, Option C): an `id="…"` attribute defines an
//! [`Kind::HtmlId`] node (`html:<relpath>#id:<name>`), enclosed by the file's
//! `document` node, with a line-granular def — the same node shape css selectors
//! get. Two payoffs follow from owning the id:
//!
//! - a same-named CSS `#id` selector joins the `html_id` via the existing
//!   `corresponds_to` edge ([`correspondence_edges`]) — a symmetric
//!   "same identifier, two languages" reading, **not** a usage edge.
//! - a lone `html_id` (a React mount point with no CSS rule) is normal, not
//!   dead; a lone `css_id` is the dead-selector signal `check_css` already mines.
//!
//! Pure functions over a [`parse_elements`](super::parse) element list plus a
//! [`CssIdLookup`] the caller constructs (cross-producer wiring is Phase 4). The
//! per-file [`HtmlIdIndex`] this builds is also the fragment-resolution anchor
//! set Phase-2 link resolution (`super::links`) looks `href="#frag"` up in.

use kenn_model::id::html::html_id;
use kenn_model::{DefRecord, EdgeProperties, EdgeRecord, Kind, Language, ShortId, SymbolRecord};

use super::links::HtmlIds;
use super::parse::{Attr, Element};

/// The `html_id` nodes (plus their defs and `defined_in` edges) extracted from
/// one file, and the per-file id index keyed by bare id name.
#[derive(Debug, Default)]
pub struct HtmlIdNodes {
    pub symbols: Vec<SymbolRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    /// Bare id name → its `html_id` node id, for this file (fragment anchors +
    /// css-id correspondence).
    pub index: HtmlIdIndex,
}

/// One file's `id="…"` allocations: bare name → `html_id` node id, in document
/// order. Small per file, so lookups are linear.
#[derive(Debug, Default, Clone)]
pub struct HtmlIdIndex {
    entries: Vec<(String, ShortId)>,
}

impl HtmlIdIndex {
    /// The `html_id` node for a bare id `name`, if the file defines it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<ShortId> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| *id)
    }

    /// `(name, node id)` pairs in document order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, ShortId)> {
        self.entries.iter().map(|(n, id)| (n.as_str(), *id))
    }

    fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|(n, _)| n == name)
    }
}

/// Lookup over the CSS `#id` selectors mined by `index-css`: a bare id name →
/// its `css_id` node id, when one exists. Constructed by the caller; the
/// cross-producer wiring (querying the building store) is Phase 4.
pub trait CssIdLookup {
    /// The `css_id` node whose bare id name equals `name`, if one exists.
    fn css_id(&self, name: &str) -> Option<ShortId>;
}

/// Emit an `html_id` node for each distinct `id="…"` attribute in document order
/// (task 4.1). Each node is enclosed by the file's `document` node (`doc_sym`),
/// carries a line-granular def in `file_id`, and a `defined_in` edge — the css
/// selector node shape. A duplicate id (first wins) is folded to one node.
pub fn html_id_nodes(
    elements: &[Element],
    relpath: &str,
    doc_sym: ShortId,
    file_id: ShortId,
    ids: &mut HtmlIds,
) -> HtmlIdNodes {
    let mut out = HtmlIdNodes::default();
    for el in elements {
        let Some(attr) = id_attr(el) else { continue };
        let name = attr.value.trim();
        if name.is_empty() || out.index.contains(name) {
            continue;
        }
        let sym = ids.mint();
        out.index.entries.push((name.to_string(), sym));
        out.symbols
            .push(html_id_symbol(sym, relpath, name, doc_sym));
        out.defs.push(def(sym, file_id, el.line));
        out.edges.push(EdgeRecord {
            src_id: sym,
            target_id: doc_sym,
            properties: EdgeProperties::DefinedIn,
        });
    }
    out
}

/// `CorrespondsTo` edges joining each `html_id` to a same-named `css_id` (task
/// 4.3). A lone `html_id` (no matching `css_id`, e.g. a JS mount point) emits
/// nothing — it stays an uncorresponded node. The edge is symmetric ("same
/// identifier, two languages"): the reader surfaces it on either endpoint.
#[must_use]
pub fn correspondence_edges(index: &HtmlIdIndex, css: &dyn CssIdLookup) -> Vec<EdgeRecord> {
    index
        .iter()
        .filter_map(|(name, html_sym)| {
            css.css_id(name).map(|css_sym| EdgeRecord {
                src_id: html_sym,
                target_id: css_sym,
                properties: EdgeProperties::CorrespondsTo {
                    source: kenn_model::IsomorphismSource::AutoInferred,
                    generator: String::new(),
                    canonical: 0,
                },
            })
        })
        .collect()
}

/// The first `id` attribute of an element (ASCII-folded name), if present.
fn id_attr(el: &Element) -> Option<&Attr> {
    el.attrs.iter().find(|a| a.name == "id")
}

/// The `html_id` node: `Kind::HtmlId`, `html:<relpath>#id:<name>`, enclosed by
/// the file's document node. The bare id is its display name.
fn html_id_symbol(id: ShortId, relpath: &str, name: &str, doc_sym: ShortId) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: crate::pubid::floor(&html_id(relpath, name).into_string()),
        language: Language::Html,
        pkg_id: 0,
        kind: Kind::HtmlId,
        name: name.to_string(),
        enclosing_sym_id: doc_sym,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

/// A line-granular def (the id'd element's start-tag line).
fn def(sym_id: ShortId, file_id: ShortId, line: u32) -> DefRecord {
    DefRecord {
        sym_id,
        file_id,
        start_line: line,
        start_col: 0,
        end_line: line,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse::parse_elements;
    use kenn_model::compose_short_id;
    use std::collections::HashMap;

    fn doc() -> ShortId {
        compose_short_id(Language::Html, 1)
    }
    fn file() -> ShortId {
        compose_short_id(Language::Html, 2)
    }

    fn nodes(html: &str, relpath: &str) -> HtmlIdNodes {
        let els = parse_elements(html);
        let mut ids = HtmlIds::new(1000);
        html_id_nodes(&els, relpath, doc(), file(), &mut ids)
    }

    /// A name → `css_id` map for correspondence tests.
    struct CssIds(HashMap<String, ShortId>);
    impl CssIdLookup for CssIds {
        fn css_id(&self, name: &str) -> Option<ShortId> {
            self.0.get(name).copied()
        }
    }
    fn css(pairs: &[(&str, u32)]) -> CssIds {
        CssIds(
            pairs
                .iter()
                .map(|(n, k)| ((*n).to_string(), compose_short_id(Language::Css, *k)))
                .collect(),
        )
    }

    #[test]
    fn id_yields_html_id_node_with_typed_pub_id() {
        let out = nodes(r#"<div id="root">hi</div>"#, "page.html");
        assert_eq!(out.symbols.len(), 1);
        let s = &out.symbols[0];
        assert_eq!(s.kind, Kind::HtmlId);
        assert_eq!(s.pub_id, "html:page.html#id:root");
        assert_eq!(s.name, "root");
        assert_eq!(s.enclosing_sym_id, doc());
        // a line-granular def + a defined_in edge to the document.
        assert_eq!(out.defs.len(), 1);
        assert_eq!(out.defs[0].file_id, file());
        assert_eq!(out.edges.len(), 1);
        assert_eq!(out.edges[0].target_id, doc());
        assert_eq!(out.edges[0].properties, EdgeProperties::DefinedIn);
        assert_eq!(out.index.get("root"), Some(out.symbols[0].id));
    }

    #[test]
    fn hostile_relpath_yields_a_shell_safe_pub_id() {
        // A Jazzy-style doc filename with brackets — the exact shape that
        // panicked the writer's shell-safe assert on a real repo (Alamofire).
        let out = nodes(
            r#"<div id="root">hi</div>"#,
            "docs/[ServerTrustEvaluating].html",
        );
        let s = &out.symbols[0];
        assert!(
            s.pub_id.chars().all(kenn_model::shell_safe::is_safe),
            "pub_id must be shell-safe: {}",
            s.pub_id
        );
        assert_eq!(s.pub_id, "html:docs/_ServerTrustEvaluating_.html#id:root");
    }

    #[test]
    fn two_ids_in_a_file_are_distinct() {
        let out = nodes(
            r#"<div id="root"></div><header id="top"></header>"#,
            "page.html",
        );
        assert_eq!(out.symbols.len(), 2);
        let ids: Vec<ShortId> = out.symbols.iter().map(|s| s.id).collect();
        assert_ne!(ids[0], ids[1]);
        assert_eq!(out.index.get("root"), Some(ids[0]));
        assert_eq!(out.index.get("top"), Some(ids[1]));
        assert!(out.index.get("missing").is_none());
    }

    #[test]
    fn duplicate_id_folds_to_one_node() {
        // Malformed but real: two id="x" → one node (first wins), like css dedup.
        let out = nodes(r#"<div id="x"></div><span id="x"></span>"#, "page.html");
        assert_eq!(out.symbols.len(), 1);
    }

    #[test]
    fn matching_html_and_css_ids_correspond() {
        let out = nodes(r#"<header id="header"></header>"#, "page.html");
        let edges = correspondence_edges(&out.index, &css(&[("header", 7)]));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, out.symbols[0].id);
        assert_eq!(edges[0].target_id, compose_short_id(Language::Css, 7));
        assert!(matches!(
            edges[0].properties,
            EdgeProperties::CorrespondsTo { .. }
        ));
    }

    #[test]
    fn lone_html_id_stays_uncorresponded() {
        // A React mount `<div id="root">` with no `#root` selector → no edge.
        let out = nodes(r#"<div id="root"></div>"#, "page.html");
        let edges = correspondence_edges(&out.index, &css(&[("other", 7)]));
        assert!(edges.is_empty());
    }

    #[test]
    fn only_matching_names_correspond() {
        let out = nodes(
            r#"<div id="root"></div><nav id="header"></nav>"#,
            "page.html",
        );
        // Only `header` has a css_id; `root` (a mount) stays lone.
        let edges = correspondence_edges(&out.index, &css(&[("header", 7)]));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, out.index.get("header").unwrap());
    }
}
