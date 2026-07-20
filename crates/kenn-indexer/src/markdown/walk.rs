//! Phase-2 markdown walk: turn one file's body + phase-1 collect into
//! `kenn_model` records — a `document` symbol, a `section` symbol per heading
//! (nested via `enclosing_sym_id` and `defined_in` edges), per-symbol
//! definition ranges, a `contains` edge to the file, and section prose as
//! docs records (FTS + embeddings).
//!
//! The `document` and each `section` are symbol-space nodes (design D10) so
//! link edges target them unambiguously; the `FileRecord` is bookkeeping for
//! the files table + change detection.

use kenn_model::id::md::{document_id, section_id};
use kenn_model::{
    compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, ShortId,
    SymbolDocsRecord, SymbolRecord,
};

use super::collect::CollectedFile;
use super::discover::DiscoveredMarkdown;

/// Allocates markdown `short_id`s in the `Markdown` partition, with separate
/// file and symbol counters (mirroring the SCIP `IdRegistry`). One instance
/// per markdown ingest so ids stay unique across the whole corpus.
#[derive(Debug)]
pub struct MarkdownIds {
    next_file: u32,
    next_symbol: u32,
}

impl Default for MarkdownIds {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownIds {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_file: 1,
            next_symbol: 1,
        }
    }

    fn file_id(&mut self) -> ShortId {
        let id = compose_short_id(Language::Markdown, self.next_file);
        self.next_file += 1;
        id
    }

    fn symbol_id(&mut self) -> ShortId {
        let id = compose_short_id(Language::Markdown, self.next_symbol);
        self.next_symbol += 1;
        id
    }

    /// Mint a fresh symbol id — used by ingest to materialize external stub
    /// nodes for dangling links in the same `Markdown` partition.
    #[must_use]
    pub fn mint_symbol(&mut self) -> ShortId {
        self.symbol_id()
    }
}

/// The records produced for one markdown file (the sibling-producer analogue
/// of the SCIP path's `TransformedDocument`).
#[derive(Debug)]
pub struct MarkdownRecords {
    pub file: FileRecord,
    pub symbols: Vec<SymbolRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    pub docs: Vec<SymbolDocsRecord>,
}

/// Walk one markdown file into records. `enclosing_module` is the `Kind::Module`
/// node for the directory the file sits in (built once across the corpus by the
/// ingest pass); the document is a `defined_in` member of it and the module
/// `contains` the file.
#[must_use]
pub fn walk_markdown(
    file: &DiscoveredMarkdown,
    content: &str,
    collected: &CollectedFile,
    ids: &mut MarkdownIds,
    enclosing_module: ShortId,
) -> MarkdownRecords {
    let lines: Vec<&str> = content.lines().collect();
    let total = u32::try_from(lines.len()).unwrap_or(u32::MAX).max(1);

    let file_id = ids.file_id();
    let doc_sym = ids.symbol_id();
    let mut out = MarkdownRecords {
        file: FileRecord {
            id: file_id,
            path: file.relpath.clone(),
            language: Language::Markdown,
            test: false,
            external: false,
            content_hash: xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
        },
        symbols: Vec::new(),
        defs: Vec::new(),
        edges: Vec::new(),
        docs: Vec::new(),
    };

    // Document node (the file-as-node, link target for the whole file),
    // enclosed by its directory module.
    let doc_name = collected
        .frontmatter
        .title
        .clone()
        .unwrap_or_else(|| stem(&file.relpath));
    out.symbols.push(symbol(
        doc_sym,
        &document_id(&file.label, &file.relpath).into_string(),
        Kind::Document,
        md_display_name(&doc_name),
        enclosing_module,
    ));
    out.defs.push(def(doc_sym, file_id, 1, total));
    // The module owns the file row (`contains`, the only file-targeting edge);
    // the document is a `defined_in` member of the module (drives list_in_scope).
    out.edges.push(EdgeRecord {
        src_id: enclosing_module,
        target_id: file_id,
        properties: EdgeProperties::Contains,
    });
    out.edges.push(EdgeRecord {
        src_id: doc_sym,
        target_id: enclosing_module,
        properties: EdgeProperties::DefinedIn,
    });

    // Section nodes, nested via a level stack.
    let headings = &collected.headings;
    let mut stack: Vec<(u8, ShortId)> = Vec::new();
    for (i, h) in headings.iter().enumerate() {
        let sym = ids.symbol_id();
        while stack.last().is_some_and(|(lvl, _)| *lvl >= h.level) {
            stack.pop();
        }
        let parent = stack.last().map_or(doc_sym, |(_, id)| *id);
        stack.push((h.level, sym));

        // Headings after this one (non-panicking; empty at the tail).
        let rest = headings.get(i + 1..).unwrap_or(&[]);
        // Full extent: to the line before the next same-or-higher heading.
        let extent_end = rest
            .iter()
            .find(|n| n.level <= h.level)
            .map_or(total, |n| n.line.saturating_sub(1));

        out.symbols.push(symbol(
            sym,
            &section_id(&file.label, &file.relpath, &h.slug).into_string(),
            Kind::Section,
            md_display_name(&h.text),
            parent,
        ));
        out.defs.push(def(sym, file_id, h.line, extent_end));
        // child --defined_in--> parent (drives list_in_scope on the parent).
        out.edges.push(EdgeRecord {
            src_id: sym,
            target_id: parent,
            properties: EdgeProperties::DefinedIn,
        });
        // Immediate prose (up to the next heading of any level → excludes
        // nested subsections) → FTS + embeddings.
        let prose = immediate_prose(&lines, h.line, rest.first().map(|n| n.line));
        if !prose.is_empty() {
            out.docs.push(SymbolDocsRecord {
                sym_id: sym,
                sig: String::new(),
                doc: prose,
            });
        }
    }

    out
}

/// Markdown ingester's grammar-appropriate name cleaning: a heading or title may
/// wrap text in inline-code backticks (a heading like "Using kenn analyze" with
/// the command in a code span), which are code-fence delimiters, not part of the
/// text. Strip them so the section / document name is plain text — the DB never
/// stores a backtick (the store debug-asserts this invariant). SCIP unwraps its
/// own escaping differently; there is no correct universal rule, so each ingester
/// owns its handling.
fn md_display_name(text: &str) -> String {
    if text.contains('`') {
        text.replace('`', "")
    } else {
        text.to_string()
    }
}

fn symbol(
    id: ShortId,
    pub_id: &str,
    kind: Kind,
    name: String,
    enclosing_sym_id: ShortId,
) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: crate::pubid::floor(pub_id),
        language: Language::Markdown,
        pkg_id: 0,
        kind,
        name,
        enclosing_sym_id,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn def(sym_id: ShortId, file_id: ShortId, start_line: u32, end_line: u32) -> DefRecord {
    DefRecord {
        sym_id,
        file_id,
        start_line,
        start_col: 0,
        end_line,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

/// Immediate prose of a heading: lines from just after its heading line
/// (`heading_line`, 1-based) up to the next heading of *any* level
/// (`next_line`), so nested subsections are excluded.
fn immediate_prose(lines: &[&str], heading_line: u32, next_line: Option<u32>) -> String {
    // `heading_line` (1-based) is the 0-based index of the line *after* it.
    let start = heading_line as usize;
    let end = next_line
        .map_or(lines.len(), |l| (l as usize).saturating_sub(1))
        .min(lines.len());
    lines
        .get(start..end)
        .unwrap_or(&[])
        .join("\n")
        .trim()
        .to_string()
}

fn stem(relpath: &str) -> String {
    let file = relpath.rsplit('/').next().unwrap_or(relpath);
    file.strip_suffix(".md")
        .or_else(|| file.strip_suffix(".markdown"))
        .unwrap_or(file)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::collect::collect;
    use std::path::PathBuf;

    fn disc(label: &str, relpath: &str) -> DiscoveredMarkdown {
        DiscoveredMarkdown {
            abs_path: PathBuf::from(relpath),
            label: label.into(),
            relpath: relpath.into(),
            in_repo: true,
        }
    }

    fn walk(content: &str) -> MarkdownRecords {
        let file = disc("workspace", "docs/auth.md");
        let collected = collect(content);
        let mut ids = MarkdownIds::new();
        let module = ids.mint_symbol(); // stand-in for the dir module
        walk_markdown(&file, content, &collected, &mut ids, module)
    }

    #[test]
    #[expect(
        clippy::many_single_char_names,
        reason = "a–d mirror the single-letter headings under test"
    )]
    fn builds_nested_heading_tree() {
        let r = walk("# A\n## B\n### C\n## D\n");
        // 1 document + 4 sections
        assert_eq!(r.symbols.len(), 5);
        let by_pub = |suffix: &str| {
            r.symbols
                .iter()
                .find(|s| s.pub_id.ends_with(suffix))
                .unwrap()
        };
        let doc = &r.symbols[0];
        assert_eq!(doc.kind, Kind::Document);
        let a = by_pub("#a");
        let b = by_pub("#b");
        let c = by_pub("#c");
        let d = by_pub("#d");
        assert_eq!(a.kind, Kind::Section);
        assert_eq!(a.enclosing_sym_id, doc.id); // top section under document
        assert_eq!(b.enclosing_sym_id, a.id); // ## under #
        assert_eq!(c.enclosing_sym_id, b.id); // ### under ##
        assert_eq!(d.enclosing_sym_id, a.id); // ## D back under # A, not B
    }

    #[test]
    fn emits_defined_in_edges_and_contains() {
        let r = walk("# A\n## B\n");
        let doc = r.symbols[0].id;
        let contains = r
            .edges
            .iter()
            .filter(|e| matches!(e.properties, EdgeProperties::Contains))
            .count();
        let defined_in = r
            .edges
            .iter()
            .filter(|e| matches!(e.properties, EdgeProperties::DefinedIn))
            .count();
        assert_eq!(contains, 1); // module contains the file
        assert_eq!(defined_in, 3); // document→module, A→doc, B→A
        assert_eq!(r.edges[0].target_id, r.file.id); // contains targets the file
        assert_eq!(r.file.id, r.defs[0].file_id);
        assert_eq!(r.defs[0].sym_id, doc); // first def is the document
    }

    #[test]
    fn section_extent_and_immediate_prose() {
        // # A spans to EOF; ## B nested; A's immediate prose excludes B's body.
        let r = walk("# A\nintro for A\n## B\nbody of B\n");
        let a_def = &r.defs[1]; // doc=0, A=1
        assert_eq!(a_def.start_line, 1);
        assert_eq!(a_def.end_line, 4); // A extends over B to EOF
        let b_def = &r.defs[2];
        assert_eq!(b_def.start_line, 3);
        // A's docs = "intro for A" (not B's body); B's docs = "body of B".
        let a_doc = r.docs.iter().find(|d| d.sym_id == a_def.sym_id).unwrap();
        assert_eq!(a_doc.doc, "intro for A");
        let b_doc = r.docs.iter().find(|d| d.sym_id == b_def.sym_id).unwrap();
        assert_eq!(b_doc.doc, "body of B");
    }

    #[test]
    fn document_name_prefers_title() {
        let r = walk("---\ntitle: My Title\n---\n# H\n");
        assert_eq!(r.symbols[0].name, "My Title");
        // heading H is at file line 4 (after the 3-line frontmatter).
        assert_eq!(r.defs[1].start_line, 4);
    }

    #[test]
    fn heading_inline_code_backticks_stripped_from_section_name() {
        // A heading with an inline-code span: the section name is plain text —
        // the DB forbids a stored backtick (markdown backticks are code fences).
        let r = walk("# Using `kenn analyze`\n");
        let section = r.symbols.iter().find(|s| s.kind == Kind::Section).unwrap();
        assert_eq!(section.name, "Using kenn analyze");
        assert!(!section.name.contains('`'));
    }

    #[test]
    fn document_title_backticks_stripped_from_name() {
        let r = walk("---\ntitle: The `foo` API\n---\n# H\n");
        assert_eq!(r.symbols[0].name, "The foo API");
        assert!(!r.symbols[0].name.contains('`'));
    }
}
