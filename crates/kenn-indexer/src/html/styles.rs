//! HTML inline `<style>` extraction (design D1 Tier 3, Phase 3).
//!
//! An inline `<style>` block is not a stylesheet *file*, but a CSS selector is a
//! CSS node wherever its text lives, and `index-css`'s node kinds
//! (`CssClass`/`CssId`/`CssVar`) are already shared across languages (design D6).
//! So each selector an inline block defines reuses the **same CSS extractor**
//! ([`collect_atoms`](crate::css)) and is emitted as a shared CSS node under the
//! **`css:` prefix** with the HTML file as its relpath
//! (`css:<relpath>#class:<name>`, `#id:`, `#var:`) — the prefix says "CSS node",
//! the relpath records the HTML owner. Using `css:` (not `html:`) keeps an inline
//! `#hero` selector's id (`css:page.html#id:hero`) **distinct** from the file's
//! `html_id` for `id="hero"` (`html:page.html#id:hero`), so the two co-exist and
//! *correspond* instead of colliding on one pub-id. The `ShortId` is still minted
//! in the HTML producer's id space (`fetch_symbol` keys on the language column +
//! pub-id, not the `ShortId` partition). Because the node carries `Kind::CssClass`
//! and a `#class:` pub-id fragment, it lands in the **same shared class registry**
//! the store serves — so a `class="hero"` anywhere can resolve to it.
//!
//! The CSS extractor's line positions are 0-based and relative to the block's
//! text; we rebase each by the block's `base_line` so a def lands on its true
//! HTML line (the markdown fenced-code pattern). Pure over a
//! [`style_blocks`](super::parse::style_blocks) list; pipeline wiring is Phase 4.
//!
//! Inline `<script>`, event-handler attributes (`onclick=`), and inline `style=`
//! declarations are **not** indexed (task 5.2): they need the separate JS/TS
//! pipeline and are out of scope. This module only consumes `<style>` blocks, so
//! none of those reach the graph here.

use std::collections::HashSet;

use kenn_model::id::css::selector_id;
use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, Language, ShortId, SymbolDocsRecord, SymbolRecord,
};

use crate::css::{collect_atoms, def, kind_of, preceding_comment, selector_text, symbol};

use super::links::HtmlIds;
use super::parse::StyleBlock;

/// The CSS nodes (plus their defs, `defined_in` edges, and docs) extracted from
/// one file's inline `<style>` blocks. Shape mirrors the `.css` producer, but
/// every node is owned by the HTML `document` (`doc_sym`).
#[derive(Debug, Default)]
pub struct InlineStyleNodes {
    pub symbols: Vec<SymbolRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    pub docs: Vec<SymbolDocsRecord>,
}

/// Extract the shared CSS nodes defined by a file's inline `<style>` blocks (task
/// 5.1). Each selector atom becomes a `css_class`/`css_id`/`css_var` node with an
/// `html:<relpath>#<type>:<name>` native id, enclosed by the file's `document`
/// node (`doc_sym`), with a line-granular def in `file_id` rebased by the block's
/// `base_line`. Atoms are deduped by pub-id across all blocks (a class repeated
/// in several rules/blocks is one node, keeping the first line seen) — the same
/// dedup the `.css` path applies. Ids come from the shared per-file [`HtmlIds`]
/// allocator, so inline-style node ids never collide with `html_id`/stub ids.
pub fn inline_style_nodes(
    blocks: &[StyleBlock],
    relpath: &str,
    doc_sym: ShortId,
    file_id: ShortId,
    ids: &mut HtmlIds,
) -> InlineStyleNodes {
    let mut out = InlineStyleNodes::default();
    let mut seen: HashSet<String> = HashSet::new();
    for block in blocks {
        let Some(atoms) = collect_atoms(&block.css) else {
            continue;
        };
        let lines: Vec<&str> = block.css.lines().collect();
        for atom in atoms {
            let pub_id = crate::pubid::floor(
                &selector_id(Language::Css, relpath, atom.kind, &atom.name).into_string(),
            );
            if !seen.insert(pub_id.clone()) {
                continue; // first def of this selector in the file wins
            }
            let sym = ids.mint();
            // The immediately-preceding `/* … */` comment feeds FTS + embeddings,
            // exactly as the `.css` path does (the value is in the prose).
            let doc = preceding_comment(&lines, atom.line as usize);
            if !doc.is_empty() {
                out.docs.push(SymbolDocsRecord {
                    sym_id: sym,
                    sig: selector_text(atom.kind, &atom.name),
                    doc,
                });
            }
            // Rebase the extractor's 0-based block line onto the HTML file: the
            // block's content begins on `base_line`, so atom line `n` → that line.
            let html_line = block.base_line.saturating_add(atom.line);
            out.symbols.push(symbol(
                sym,
                pub_id,
                Language::Css,
                kind_of(atom.kind),
                atom.name,
                doc_sym,
            ));
            out.defs.push(def(sym, file_id, html_line, html_line));
            out.edges.push(EdgeRecord {
                src_id: sym,
                target_id: doc_sym,
                properties: EdgeProperties::DefinedIn,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse::style_blocks;
    use kenn_model::{compose_short_id, Kind};

    fn doc() -> ShortId {
        compose_short_id(Language::Html, 1)
    }
    fn file() -> ShortId {
        compose_short_id(Language::Html, 2)
    }

    fn nodes(html: &str, relpath: &str) -> InlineStyleNodes {
        let blocks = style_blocks(html);
        let mut ids = HtmlIds::new(1000);
        inline_style_nodes(&blocks, relpath, doc(), file(), &mut ids)
    }

    /// Task 5.1: `<style>.hero{}</style>` defines a `css_class` node with the
    /// HTML-owned native id at its line, registered in the shared class registry
    /// (it carries `Kind::CssClass` + a `#class:` fragment + the bare short name,
    /// which is exactly what the store's class registry keys on).
    #[test]
    fn inline_style_defines_html_owned_css_class() {
        let out = nodes("<style>.hero { }</style>", "page.html");
        assert_eq!(out.symbols.len(), 1);
        let s = &out.symbols[0];
        assert_eq!(s.kind, Kind::CssClass);
        assert_eq!(s.pub_id, "css:page.html#class:hero");
        assert_eq!(s.name, "hero"); // short name → resolvable by `symbols_by_short_name`
        assert_eq!(s.language, Language::Css);
        assert_eq!(s.enclosing_sym_id, doc());
        // line-granular def + a defined_in edge to the document.
        assert_eq!(out.defs.len(), 1);
        assert_eq!(out.defs[0].file_id, file());
        assert_eq!(out.defs[0].start_line, 1);
        assert_eq!(out.defs[0].end_line, 1);
        assert_eq!(out.edges.len(), 1);
        assert_eq!(out.edges[0].target_id, doc());
        assert_eq!(out.edges[0].properties, EdgeProperties::DefinedIn);
    }

    /// Positions rebase by the block's base line: a multi-line `<style>` that
    /// starts partway down the file places each selector on its true HTML line.
    #[test]
    fn positions_rebase_by_block_base_line() {
        let html =
            "<h1>title</h1>\n<p>lead</p>\n<style>\n.a { color: red }\n.b { color: blue }\n</style>";
        let out = nodes(html, "page.html");
        let line_of = |name: &str| {
            let s = out.symbols.iter().find(|s| s.name == name).unwrap();
            out.defs
                .iter()
                .find(|d| d.sym_id == s.id)
                .unwrap()
                .start_line
        };
        // <style> opens on line 3; `.a` is line 4, `.b` is line 5.
        assert_eq!(line_of("a"), 4);
        assert_eq!(line_of("b"), 5);
    }

    /// Ids and vars from an inline block reuse the shared kinds under the `css:`
    /// prefix (HTML file as relpath); an id and a same-named class never collide
    /// (the `#id:`/`#class:` type segment), and the preceding comment feeds the
    /// docs record.
    #[test]
    fn inline_block_emits_ids_vars_and_docs() {
        let html = "<style>\n\
                    :root { --brand: #36f }\n\
                    /* Primary hero */\n\
                    .hero { color: red }\n\
                    #hero { color: blue }\n\
                    </style>";
        let out = nodes(html, "page.html");
        let by_pub = |p: &str| out.symbols.iter().find(|s| s.pub_id == p);
        assert!(matches!(
            by_pub("css:page.html#var:--brand").map(|s| s.kind),
            Some(Kind::CssVar)
        ));
        let class = by_pub("css:page.html#class:hero").expect("class node");
        let id = by_pub("css:page.html#id:hero").expect("id node");
        // An inline `#hero` selector is a shared `css_id` node (not an `html_id`,
        // which comes from an `id="…"` attribute — see `ids.rs`).
        assert_eq!(class.kind, Kind::CssClass);
        assert_eq!(id.kind, Kind::CssId);
        assert_ne!(class.pub_id, id.pub_id);
        // the `/* Primary hero */` comment attaches to the `.hero` class.
        let doc = out.docs.iter().find(|d| d.sym_id == class.id).expect("doc");
        assert_eq!(doc.doc, "Primary hero");
        assert_eq!(doc.sig, ".hero");
    }

    /// A class repeated across rules/blocks folds to one node (first line wins),
    /// matching the `.css` producer's per-file dedup.
    #[test]
    fn duplicate_selector_folds_to_one_node() {
        let html = "<style>.btn {}</style><style>.btn { color: red }</style>";
        let out = nodes(html, "page.html");
        let btn = out.symbols.iter().filter(|s| s.name == "btn").count();
        assert_eq!(btn, 1);
    }

    /// Task 5.2: inline `<script>`, an `onclick=` handler, and an inline `style=`
    /// declaration produce **no** nodes — only `<style>` blocks are indexed.
    #[test]
    fn script_handlers_and_inline_style_attr_yield_no_nodes() {
        let html = r#"<button onclick="go()" style="color:red">x</button>
                      <script>const x = 1; const cls = "ghost";</script>"#;
        let out = nodes(html, "page.html");
        assert!(out.symbols.is_empty());
        assert!(out.defs.is_empty());
        assert!(out.edges.is_empty());
    }
}
