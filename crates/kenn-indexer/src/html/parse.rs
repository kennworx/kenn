//! WHATWG HTML parse → a flat, line-tagged element list (the keystone's
//! load-bearing internal API; later tiers consume [`Element`]).
//!
//! html5ever's `RcDom` builds a fully WHATWG-conformant tree (implied/optional
//! close, raw-text `<script>`/`<style>`, foreign content, malformed recovery)
//! but carries **no source positions**. We chose a position-tracking `TreeSink`
//! over a raw `TokenSink`: a raw tokenizer cannot handle raw-text elements,
//! because it is the *tree builder* that flips the tokenizer into the
//! RAWTEXT/script-data state — so `<script>if(a<b)</script>`'s `<b` would be
//! mis-tokenized as a start tag without the tree.
//!
//! [`LineSink`] wraps an inner `RcDom`, delegating every tree operation to it
//! (reusing its battle-tested mechanics) and overriding only two methods:
//! `set_current_line` (the tokenizer's line signal) and `create_element` (record
//! the current line against the new node's pointer identity). A post-parse walk
//! over the finished tree yields nesting for free and looks each element's line
//! up from that map.
//!
//! **Line granularity (design D3):** an element's `line` is where its start tag
//! *closes* (`>`) — equal to the tag's line for the common single-line case. The
//! tokenizer counts newlines in *every* state, including inside a multi-line
//! `<script>`, so elements *after* a raw-text block report the right line — the
//! risk the design flagged. html5ever has no per-attribute positions, so every
//! attribute of one element shares that element's line.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{parse_document, Attribute, ExpandedName, ParseOpts, QualName};
use markup5ever_rcdom::{Handle, Node, NodeData, RcDom};

/// One attribute of an [`Element`]: name, value, and source line. At line
/// granularity the line equals the enclosing element's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    /// Local attribute name, ASCII-case-folded for HTML (`CLASS` → `class`).
    pub name: String,
    /// Attribute value verbatim (case preserved); empty for a valueless attr.
    pub value: String,
    /// 1-based source line.
    pub line: u32,
}

/// One inline `<style>` block's raw CSS text and the HTML source line its
/// content begins on (Tier 3, design D6). `base_line` is the `<style>` element's
/// start-tag line: the block's text starts immediately after `>`, so the CSS
/// extractor's 0-based line `n` maps to HTML line `base_line + n` — the markdown
/// fenced-code rebasing pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleBlock {
    /// Raw CSS text inside the `<style>` element, verbatim (a raw-text element,
    /// so its content is never parsed as markup).
    pub css: String,
    /// 1-based HTML source line the block's content begins on.
    pub base_line: u32,
}

/// One parsed HTML element in document (pre-order) position: its tag, its
/// attributes, the source line of its start tag, and the index of its nearest
/// enclosing element in the same list (`None` at top level). This phase emits
/// **no** edges from attributes — it is the extraction surface later tiers call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// Local tag name, ASCII-case-folded for HTML (`DIV` → `div`).
    pub tag: String,
    /// Attributes in source order.
    pub attrs: Vec<Attr>,
    /// 1-based source line of the element's start tag (`>`).
    pub line: u32,
    /// Index into the returned slice of the nearest enclosing element.
    pub parent: Option<usize>,
}

/// Parse a complete HTML document and return its elements in document order,
/// each tagged with tag/attributes/line and a parent-element index. Always
/// succeeds: html5ever recovers from any malformed input (design corpus).
#[must_use]
pub fn parse_elements(html: &str) -> Vec<Element> {
    let sink = LineSink::default();
    let parsed = parse_document(sink, ParseOpts::default()).one(html);
    let mut out = Vec::new();
    let lines = parsed.lines.borrow();
    for child in parsed.dom.document.children.borrow().iter() {
        walk(child, None, &lines, &mut out);
    }
    out
}

/// Parse a complete HTML document and return its inline `<style>` blocks in
/// document order: each block's raw CSS text plus the HTML line its content
/// begins on (Tier 3, design D6). `<style>` is a raw-text element, so its content
/// is a single text child the WHATWG tree builder never parses as markup — `<` in
/// the CSS stays text. `base_line` is the `<style>` element's start-tag line; the
/// content starts right after `>`, so the CSS extractor's 0-based line `n` maps
/// to HTML line `base_line + n`.
#[must_use]
pub fn style_blocks(html: &str) -> Vec<StyleBlock> {
    let sink = LineSink::default();
    let parsed = parse_document(sink, ParseOpts::default()).one(html);
    let mut out = Vec::new();
    let lines = parsed.lines.borrow();
    for child in parsed.dom.document.children.borrow().iter() {
        walk_styles(child, &lines, &mut out);
    }
    out
}

/// Depth-first walk collecting each `<style>` element's text content + base line.
/// A `<style>` is raw-text (no element descendants), so we read its text children
/// and do not recurse into it; everything else recurses (incl. `<template>`).
fn walk_styles(node: &Handle, lines: &HashMap<*const Node, u32>, out: &mut Vec<StyleBlock>) {
    if let NodeData::Element { name, .. } = &node.data {
        if name.local.as_ref() == "style" {
            let base_line = lines
                .get(&(std::rc::Rc::as_ptr(node)))
                .copied()
                .unwrap_or(1);
            let mut css = String::new();
            for child in node.children.borrow().iter() {
                if let NodeData::Text { contents } = &child.data {
                    css.push_str(&contents.borrow());
                }
            }
            out.push(StyleBlock { css, base_line });
            return;
        }
        if let NodeData::Element {
            template_contents, ..
        } = &node.data
        {
            if let Some(tpl) = template_contents.borrow().as_ref() {
                for child in tpl.children.borrow().iter() {
                    walk_styles(child, lines, out);
                }
            }
        }
    }
    for child in node.children.borrow().iter() {
        walk_styles(child, lines, out);
    }
}

/// Depth-first pre-order walk: append each element node (resolving its line from
/// the pointer map, default 1) and recurse into its children — and its
/// `<template>` contents — with this element as the parent.
fn walk(
    node: &Handle,
    parent: Option<usize>,
    lines: &HashMap<*const Node, u32>,
    out: &mut Vec<Element>,
) {
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let line = lines
            .get(&(std::rc::Rc::as_ptr(node)))
            .copied()
            .unwrap_or(1);
        let idx = out.len();
        out.push(Element {
            tag: name.local.to_string(),
            attrs: attrs
                .borrow()
                .iter()
                .map(|a| Attr {
                    name: a.name.local.to_string(),
                    value: a.value.to_string(),
                    line,
                })
                .collect(),
            line,
            parent,
        });
        if let NodeData::Element {
            template_contents, ..
        } = &node.data
        {
            if let Some(tpl) = template_contents.borrow().as_ref() {
                for child in tpl.children.borrow().iter() {
                    walk(child, Some(idx), lines, out);
                }
            }
        }
        for child in node.children.borrow().iter() {
            walk(child, Some(idx), lines, out);
        }
    } else {
        // Document / text / comment / doctype: not an element, but recurse so
        // element descendants of a non-element wrapper are still reached.
        for child in node.children.borrow().iter() {
            walk(child, parent, lines, out);
        }
    }
}

/// A [`TreeSink`] that wraps `RcDom` to capture per-element source lines.
/// Everything except `create_element`/`set_current_line` delegates to the inner
/// dom, so WHATWG tree construction is unchanged.
#[derive(Default)]
struct LineSink {
    dom: RcDom,
    /// The tokenizer's current line, updated by `set_current_line`.
    current_line: Cell<u32>,
    /// new-element pointer identity → the line at its creation.
    lines: RefCell<HashMap<*const Node, u32>>,
}

impl TreeSink for LineSink {
    type Output = Self;
    type Handle = Handle;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let handle = self.dom.create_element(name, attrs, flags);
        let line = self.current_line.get().max(1);
        self.lines
            .borrow_mut()
            .insert(std::rc::Rc::as_ptr(&handle), line);
        handle
    }

    fn set_current_line(&self, line_number: u64) {
        self.current_line
            .set(u32::try_from(line_number).unwrap_or(u32::MAX));
    }

    // --- everything below delegates to the inner RcDom ----------------------

    fn parse_error(&self, msg: std::borrow::Cow<'static, str>) {
        self.dom.parse_error(msg);
    }

    fn get_document(&self) -> Handle {
        self.dom.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        self.dom.elem_name(target)
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        self.dom.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        self.dom.create_pi(target, data)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        self.dom.append(parent, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        self.dom
            .append_based_on_parent_node(element, prev_element, child);
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        self.dom
            .append_doctype_to_document(name, public_id, system_id);
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        self.dom.get_template_contents(target)
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        self.dom.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.dom.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &Handle, new_node: NodeOrText<Handle>) {
        self.dom.append_before_sibling(sibling, new_node);
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        self.dom.add_attrs_if_missing(target, attrs);
    }

    fn remove_from_parent(&self, target: &Handle) {
        self.dom.remove_from_parent(target);
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        self.dom.reparent_children(node, new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        self.dom.is_mathml_annotation_xml_integration_point(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Find the first element with the given tag.
    fn tag<'a>(els: &'a [Element], t: &str) -> &'a Element {
        els.iter()
            .find(|e| e.tag == t)
            .unwrap_or_else(|| panic!("no <{t}> in {els:?}"))
    }

    /// All elements with the given tag.
    fn all<'a>(els: &'a [Element], t: &str) -> Vec<&'a Element> {
        els.iter().filter(|e| e.tag == t).collect()
    }

    /// The value of an element's named attribute, if present.
    fn attr<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
        e.attrs
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    // --- quirk corpus (task 2.4) -------------------------------------------

    #[test]
    fn void_and_open_only_tags_dont_swallow_siblings() {
        // <br>/<img>/<input> have no end tag: the <a> after them is a sibling,
        // not a child.
        let els = parse_elements(r#"<div><br><img src="a"><a href="/x">y</a></div>"#);
        let a = tag(&els, "a");
        let div = tag(&els, "div");
        let div_idx = els.iter().position(|e| std::ptr::eq(e, div)).unwrap();
        // <a> is a child of <div>, not of <br>/<img>.
        assert_eq!(a.parent, Some(div_idx));
        assert_eq!(attr(tag(&els, "img"), "src"), Some("a"));
    }

    #[test]
    fn valueless_attribute_has_empty_value() {
        let els = parse_elements("<input disabled><option selected>");
        assert_eq!(attr(tag(&els, "input"), "disabled"), Some(""));
        assert_eq!(attr(tag(&els, "option"), "selected"), Some(""));
    }

    #[test]
    fn unquoted_value_ends_at_whitespace() {
        let els = parse_elements("<a href=/foo class=btn>x</a>");
        let a = tag(&els, "a");
        assert_eq!(attr(a, "href"), Some("/foo"));
        assert_eq!(attr(a, "class"), Some("btn"));
    }

    #[test]
    fn self_closing_slash_on_non_void_is_ignored() {
        // <div/> stays OPEN: the following <span> nests inside it.
        let els = parse_elements("<div/><span>x</span>");
        let div = tag(&els, "div");
        let div_idx = els.iter().position(|e| std::ptr::eq(e, div)).unwrap();
        assert_eq!(tag(&els, "span").parent, Some(div_idx));
    }

    #[test]
    fn optional_close_li_are_siblings() {
        // <li>a<li>b → two sibling <li>, not nested.
        let els = parse_elements("<ul><li>a<li>b</ul>");
        let lis = all(&els, "li");
        assert_eq!(lis.len(), 2);
        assert_eq!(lis[0].parent, lis[1].parent, "the two <li> are siblings");
    }

    #[test]
    fn implied_tbody_is_inserted() {
        // <table><tr><td> → a tbody is auto-inserted between table and tr.
        let els = parse_elements("<table><tr><td>x</table>");
        let tbody = tag(&els, "tbody");
        let table_idx = els.iter().position(|e| e.tag == "table").unwrap();
        assert_eq!(tbody.parent, Some(table_idx));
        let tbody_idx = els.iter().position(|e| e.tag == "tbody").unwrap();
        assert_eq!(tag(&els, "tr").parent, Some(tbody_idx));
    }

    #[test]
    fn raw_text_script_content_is_not_markup() {
        // <b inside the script is text, so there is no <b> element; the script
        // itself carries no attribute.
        let els = parse_elements("<script>if (a < b) {}</script>");
        assert!(
            all(&els, "b").is_empty(),
            "<b is script text, not an element"
        );
        assert!(tag(&els, "script").attrs.is_empty());
    }

    #[test]
    fn comment_yields_no_element_or_attribute() {
        // A class= inside a comment must not surface as an element/attribute.
        let els = parse_elements(r#"<body><!-- <div class="ghost"> --><p>x</p></body>"#);
        assert!(all(&els, "div").is_empty());
        assert!(
            !els.iter().any(|e| attr(e, "class") == Some("ghost")),
            "comment content is not extracted"
        );
    }

    #[test]
    fn duplicate_attribute_first_wins() {
        let els = parse_elements(r#"<div class="a" class="b">x</div>"#);
        let div = tag(&els, "div");
        let classes: Vec<&str> = div
            .attrs
            .iter()
            .filter(|a| a.name == "class")
            .map(|a| a.value.as_str())
            .collect();
        assert_eq!(classes, ["a"], "duplicate attr: first wins");
    }

    #[test]
    fn foreign_svg_preserves_attr_case_and_self_close() {
        // SVG: viewBox keeps its camelCase; <rect/> self-closes (a sibling, not
        // a parent of following content).
        let els = parse_elements(r#"<svg viewBox="0 0 1 1"><rect/><circle/></svg>"#);
        assert_eq!(attr(tag(&els, "svg"), "viewBox"), Some("0 0 1 1"));
        let svg_idx = els.iter().position(|e| e.tag == "svg").unwrap();
        // both rect and circle are children of svg (rect did not swallow circle).
        assert_eq!(tag(&els, "rect").parent, Some(svg_idx));
        assert_eq!(tag(&els, "circle").parent, Some(svg_idx));
    }

    #[test]
    fn malformed_unclosed_div_still_extracts() {
        // Unclosed <div> at EOF: html5ever recovers, the attr is still extracted.
        let els = parse_elements(r#"<div class="btn"><span>x"#);
        assert_eq!(attr(tag(&els, "div"), "class"), Some("btn"));
        assert!(els.iter().any(|e| e.tag == "span"));
    }

    #[test]
    fn templating_braces_are_opaque_text() {
        // `{{x}}` / `${y}` are not HTML syntax — they ride through as the
        // verbatim attribute value (later tiers grade them).
        let els = parse_elements(r#"<div class="{{x}}"><a class="${y}">z</a></div>"#);
        assert_eq!(attr(tag(&els, "div"), "class"), Some("{{x}}"));
        assert_eq!(attr(tag(&els, "a"), "class"), Some("${y}"));
    }

    #[test]
    fn tag_and_attr_case_fold_value_preserved() {
        let els = parse_elements("<DIV CLASS=Btn>x</DIV>");
        let div = tag(&els, "div"); // tag folded
        assert_eq!(attr(div, "class"), Some("Btn")); // attr folded, value kept
    }

    // --- line granularity under quirks (task 2.5) --------------------------

    #[test]
    fn lines_track_across_multiline_attributes() {
        // The <a> start tag closes on line 3; its attr shares that line.
        let html = "<div>\n  <a\n    href=\"/x\">y</a>\n</div>";
        let els = parse_elements(html);
        let a = tag(&els, "a");
        assert_eq!(a.line, 3, "element line = where the start tag closes");
        assert_eq!(a.attrs[0].line, 3);
    }

    #[test]
    fn lines_stay_correct_after_implied_close_and_multiline_script() {
        // The D3 risk: a <li>-implied-close list, then a multi-line <script>,
        // then a downstream <p> — the <p> must report its true line (the
        // tokenizer counts newlines even inside the raw-text script).
        let html = "<ul>\n\
                    <li>one\n\
                    <li>two\n\
                    </ul>\n\
                    <script>\n\
                    var a = 1;\n\
                    if (a < 2) {}\n\
                    </script>\n\
                    <p id=\"after\">tail</p>\n";
        let els = parse_elements(html);
        let lis = all(&els, "li");
        assert_eq!(lis[0].line, 2);
        assert_eq!(lis[1].line, 3);
        assert_eq!(tag(&els, "script").line, 5);
        // The <p> sits on line 9, after the 4-line script — the key assertion.
        assert_eq!(tag(&els, "p").line, 9);
        assert_eq!(attr(tag(&els, "p"), "id"), Some("after"));
    }

    // --- inline <style> blocks (task 5.1) ----------------------------------

    #[test]
    fn single_line_style_block_base_line_is_its_tag_line() {
        let blocks = style_blocks("<style>.hero{}</style>");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].css, ".hero{}");
        assert_eq!(blocks[0].base_line, 1);
    }

    #[test]
    fn style_block_base_line_rebases_multiline_selectors() {
        // <style> opens on line 3 (two leading lines); its content begins on the
        // same line, so the CSS extractor's 0-based line + base_line lands each
        // selector on its true HTML line.
        let html = "<div>x</div>\n<p>y</p>\n<style>\n.a {}\n.b {}\n</style>";
        let blocks = style_blocks(html);
        assert_eq!(blocks.len(), 1);
        // base_line = the <style> tag line (3); content text starts after `>`.
        assert_eq!(blocks[0].base_line, 3);
        // text line 0 = rest of line 3 (empty), line 1 = `.a`, line 2 = `.b`.
        assert_eq!(blocks[0].css, "\n.a {}\n.b {}\n");
    }

    #[test]
    fn style_with_multiline_open_tag_base_line_is_where_it_closes() {
        // A `<style …>` whose start tag spans lines: base_line is where `>` lands
        // (line 2), since the content begins right after it.
        let html = "<style\n  type=\"text/css\">\n.hero {}\n</style>";
        let blocks = style_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].base_line, 2);
    }

    #[test]
    fn script_is_not_a_style_block() {
        // Only <style> is collected; <script> (raw text too) is ignored — inline
        // JS is out of scope (task 5.2).
        let blocks = style_blocks("<script>const x = 1;</script><style>.k{}</style>");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].css, ".k{}");
    }
}
