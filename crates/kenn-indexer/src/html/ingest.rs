//! HTML ingest: the sibling-producer pass (design D1) plus the post-code/CSS
//! barrier resolution (design D4, Phase 4).
//!
//! Two passes, mirroring the css producer:
//!
//! - [`ingest_html`] — the parallel producer. Discovers `.html`/`.htm` files,
//!   parses each with html5ever (full WHATWG tree recovery), and emits the nodes
//!   that must exist *before* any cross-producer edge can resolve: the per-file
//!   `document` node, its `html_id` nodes, and its inline-`<style>` CSS nodes
//!   (which register into the shared class registry). The connective edges defer:
//!   each file's element list + id index ride out in [`HtmlPending`].
//! - [`resolve_html`] — the post-code/CSS barrier (design D4). Once code + CSS
//!   ingest have populated the file set, the css class registry, and the
//!   `css_id` nodes, this opens a read snapshot and resolves the deferred HTML
//!   edges —
//!   links, imports, asset attachments, `html_id`↔`css_id` correspondence, and
//!   `class=` usage — against store-backed lookups.

use kenn_config::HtmlConfig;
use kenn_model::id::html::document_id;
use kenn_model::{
    compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, ShortId,
    SymbolRecord,
};
use kenn_store::api::DbError;
use kenn_store::DbReader;
use tokio::runtime::Handle;

use super::classes::{class_usage_edges, ClassRegistry};
use super::discover::{discover_html, HtmlDiscoverError};
use super::ids::{correspondence_edges, html_id_nodes, CssIdLookup, HtmlIdIndex};
use super::links::{
    anchor_link_edges, asset_link_edges, import_edges, AssetIndex, FragmentIndex, HtmlIds, StubSink,
};
use super::parse::{parse_elements, style_blocks, Element};
use super::styles::inline_style_nodes;
use crate::markdown::StoreCodeLookup;
use crate::sink::BatchSink;

#[derive(Debug, thiserror::Error)]
pub enum HtmlIngestError {
    #[error(transparent)]
    Discover(#[from] HtmlDiscoverError),
    #[error("store append failed: {0}")]
    Store(#[from] DbError),
}

/// Record counts produced by an HTML ingest, for the run report.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlCounts {
    pub files: u64,
    pub symbols: u64,
    pub defs: u64,
    pub edges: u64,
    /// Total elements parsed across all files (extraction proof, Phase 0).
    pub elements: u64,
}

/// Counts from the post-barrier HTML resolution, for the run report.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlResolveCounts {
    /// Stub `attachment`/`document` nodes minted for unresolved/asset targets.
    pub stubs: u64,
    /// Link / import / asset / correspondence / class-usage edges emitted.
    pub edges: u64,
}

/// One file's deferred state: everything the barrier needs to resolve its
/// connective edges without re-reading or re-parsing it.
struct PendingFile {
    relpath: String,
    /// The file's `document` node (the link/usage source + file fallback).
    doc_sym: ShortId,
    /// The flat element list (links, assets, class usage are read off it).
    elements: Vec<Element>,
    /// This file's `id="…"` index (enclosing-id attribution + correspondence).
    id_index: HtmlIdIndex,
}

/// State carried from the HTML producer to the post-code/CSS barrier (design D4).
/// Mirrors [`CssPending`](crate::css::CssPending): the per-file element lists and
/// id indexes plus the symbol-id high-water mark to resume stub minting from
/// (so barrier-minted stub ids never collide with the keystone's node ids).
pub struct HtmlPending {
    files: Vec<PendingFile>,
    next_sym: u32,
}

/// Discover HTML files, parse each, and emit its keystone nodes — the `document`
/// node, its `html_id` nodes, and its inline-`<style>` CSS nodes — through
/// `sink`, finishing it. The connective edges defer to [`resolve_html`]; the
/// per-file element lists + id indexes ride out in the returned [`HtmlPending`].
/// The caller gates on `config.enabled`.
pub fn ingest_html(
    config: &HtmlConfig,
    workspace_root: &std::path::Path,
    mut sink: BatchSink,
) -> Result<(HtmlCounts, HtmlPending), HtmlIngestError> {
    let mut counts = HtmlCounts::default();
    let mut next_file: u32 = 1;
    // One shared symbol allocator across the whole run: document, html_id, and
    // inline-style node ids are all disjoint, and the barrier resumes stub minting
    // from its high-water mark.
    let mut ids = HtmlIds::new(1);
    let mut pending = Vec::new();

    for file in discover_html(config, workspace_root)? {
        let Ok(content) = std::fs::read_to_string(&file.abs_path) else {
            tracing::warn!(
                target: "kenn_indexer::html",
                path = %file.abs_path.display(),
                "unreadable html file, skipped"
            );
            continue;
        };
        let elements = parse_elements(&content);
        counts.elements += elements.len() as u64;

        let file_id = compose_short_id(Language::Html, next_file);
        next_file += 1;
        let doc_sym = ids.mint();
        let total = u32::try_from(content.lines().count())
            .unwrap_or(u32::MAX)
            .max(1);

        // Keystone: the document node + its file row + the scope→file `contains`.
        sink.push_document_records(
            std::iter::once(FileRecord {
                id: file_id,
                path: file.relpath.clone(),
                language: Language::Html,
                test: false,
                external: false,
                content_hash: xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
            }),
            std::iter::once(document_node(doc_sym, &file.relpath)),
            std::iter::empty(),
            std::iter::once(def(doc_sym, file_id, 1, total)),
            std::iter::once(EdgeRecord {
                src_id: doc_sym,
                target_id: file_id,
                properties: EdgeProperties::Contains,
            }),
        )?;
        counts.files += 1;
        counts.symbols += 1;
        counts.defs += 1;
        counts.edges += 1;

        // `html_id` nodes (also the fragment-anchor / correspondence index) and
        // inline-`<style>` CSS nodes (registry members) — both must exist before
        // the barrier resolves against them.
        let id_nodes = html_id_nodes(&elements, &file.relpath, doc_sym, file_id, &mut ids);
        emit_nodes(
            &mut sink,
            &mut counts,
            id_nodes.symbols,
            id_nodes.defs,
            id_nodes.edges,
            Vec::new(),
        )?;
        let styles = inline_style_nodes(
            &style_blocks(&content),
            &file.relpath,
            doc_sym,
            file_id,
            &mut ids,
        );
        emit_nodes(
            &mut sink,
            &mut counts,
            styles.symbols,
            styles.defs,
            styles.edges,
            styles.docs,
        )?;

        pending.push(PendingFile {
            relpath: file.relpath,
            doc_sym,
            elements,
            id_index: id_nodes.index,
        });
    }
    sink.finish()?;
    Ok((
        counts,
        HtmlPending {
            files: pending,
            next_sym: ids.current(),
        },
    ))
}

/// Post-code/CSS barrier (design D4, mirrors [`resolve_css_usage`]): resolve the
/// deferred HTML edges against the building store. Opens nothing itself — the
/// caller passes the read snapshot + runtime handle (the store always holds the
/// HTML/CSS nodes, so HTML resolution runs regardless of whether code ingest
/// ran). For each file: link/import/asset edges (markdown file resolver +
/// filesystem asset check), `html_id`↔`css_id` correspondence, and `class=`
/// usage. Owns and finishes `sink`.
///
/// [`resolve_css_usage`]: crate::css::resolve_css_usage
pub fn resolve_html(
    pending: HtmlPending,
    reader: &DbReader,
    handle: &Handle,
    workspace_root: &std::path::Path,
    mut sink: BatchSink,
) -> Result<HtmlResolveCounts, HtmlIngestError> {
    let mut counts = HtmlResolveCounts::default();
    let files = StoreCodeLookup { reader, handle };
    let css_ids = StoreCssIdLookup { reader, handle };
    let registry = StoreClassRegistry { reader, handle };
    let assets = FsAssets { workspace_root };
    // The corpus-wide fragment-anchor set (`href="#frag"` resolves against it).
    let frags = FragmentIndex::new(
        pending
            .files
            .iter()
            .map(|f| (f.relpath.clone(), f.id_index.clone())),
    );
    // One shared id allocator + stub sink across all files: an asset referenced
    // from two pages collapses to a single stub (spec: deterministic reverse
    // lookup). Stub ids resume past the keystone's node high-water mark.
    let mut html_ids = HtmlIds::new(pending.next_sym);
    let mut stubs = StubSink::default();

    for file in pending.files {
        let mut edges = import_edges(
            &file.elements,
            &file.relpath,
            file.doc_sym,
            &files,
            &mut html_ids,
            &mut stubs,
        );
        edges.extend(anchor_link_edges(
            &file.elements,
            &file.relpath,
            file.doc_sym,
            &files,
            &frags,
            &assets,
            &mut html_ids,
            &mut stubs,
        ));
        edges.extend(asset_link_edges(
            &file.elements,
            &file.relpath,
            file.doc_sym,
            &assets,
            &mut html_ids,
            &mut stubs,
        ));
        edges.extend(correspondence_edges(&file.id_index, &css_ids));
        edges.extend(class_usage_edges(
            &file.elements,
            file.doc_sym,
            &file.id_index,
            &registry,
        ));
        for edge in edges {
            sink.push_edge(edge)?;
            counts.edges += 1;
        }
    }
    // Push the deduped stub nodes once (their edges already reference them; the
    // aggregate join resolves edge→symbol regardless of insertion order).
    for stub in stubs.records {
        sink.push_symbol(stub)?;
        counts.stubs += 1;
    }
    sink.finish()?;
    Ok(counts)
}

/// Push one node group (symbols/defs/edges/docs) through `sink`, accumulating the
/// report counts. The file row was already pushed with the document node.
fn emit_nodes(
    sink: &mut BatchSink,
    counts: &mut HtmlCounts,
    symbols: Vec<SymbolRecord>,
    defs: Vec<DefRecord>,
    edges: Vec<EdgeRecord>,
    docs: Vec<kenn_model::SymbolDocsRecord>,
) -> Result<(), DbError> {
    counts.symbols += symbols.len() as u64;
    counts.defs += defs.len() as u64;
    counts.edges += edges.len() as u64;
    sink.push_document_records(std::iter::empty(), symbols, docs, defs, edges)
}

/// [`CssIdLookup`] over the building store: a bare id name → a `css_id` node that
/// defines it (filtered from `symbols_by_short_name` by the `#id:` pub-id
/// fragment, excluding `html:`-owned ids so an `html_id` never corresponds to
/// itself). Mirrors markdown's `StoreCodeLookup`.
struct StoreCssIdLookup<'a> {
    reader: &'a DbReader,
    handle: &'a Handle,
}

impl CssIdLookup for StoreCssIdLookup<'_> {
    fn css_id(&self, name: &str) -> Option<ShortId> {
        self.handle
            .block_on(self.reader.symbols_by_short_name(name))
            .unwrap_or_default()
            .into_iter()
            .find(|h| h.qualified.contains("#id:") && !h.qualified.starts_with("html:"))
            .map(|h| h.id)
    }
}

/// [`ClassRegistry`] over the building store: a class name → the `css_class` node
/// ids that define it (filtered by the `#class:` pub-id fragment — the same
/// registry the css usage scan resolves against, design D6).
struct StoreClassRegistry<'a> {
    reader: &'a DbReader,
    handle: &'a Handle,
}

impl ClassRegistry for StoreClassRegistry<'_> {
    fn class_ids(&self, name: &str) -> Vec<ShortId> {
        self.handle
            .block_on(self.reader.symbols_by_short_name(name))
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.qualified.contains("#class:"))
            .map(|h| h.id)
            .collect()
    }
}

/// [`AssetIndex`] over the filesystem: an asset exists when its canonical
/// workspace-relative path is a file on disk (design: back the asset check with a
/// real `exists`, so existing assets key by canonical path and missing ones
/// dangle).
struct FsAssets<'a> {
    workspace_root: &'a std::path::Path,
}

impl AssetIndex for FsAssets<'_> {
    fn exists(&self, canonical_path: &str) -> bool {
        !canonical_path.is_empty() && self.workspace_root.join(canonical_path).is_file()
    }
}

/// The HTML file-as-node: a `document` (link target for the whole file),
/// `html:<relpath>`. The document basename is its display name.
fn document_node(id: ShortId, relpath: &str) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: crate::pubid::floor(&document_id(relpath).into_string()),
        language: Language::Html,
        pkg_id: 0,
        kind: Kind::Document,
        name: relpath.rsplit('/').next().unwrap_or(relpath).to_string(),
        enclosing_sym_id: 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_store::api::Reader;
    use std::fs;
    use tempfile::TempDir;

    fn cfg() -> HtmlConfig {
        HtmlConfig {
            enabled: true,
            roots: vec![".".into()],
            ..Default::default()
        }
    }

    /// Open a multi-thread runtime + a fresh building-store writer under `ws`.
    fn writer_for(ws: &std::path::Path) -> (tokio::runtime::Runtime, kenn_store::DbWriter) {
        let building = ws.join(".kenn/local/building");
        fs::create_dir_all(&building).unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let writer = rt
            .block_on(kenn_store::open_writer(
                &building,
                kenn_store::WriterOptions::default(),
            ))
            .expect("open_writer");
        (rt, writer)
    }

    /// A fixture `index.html` yields exactly one `document` node carrying its
    /// workspace-relative path, written through the real store (task 2.2).
    #[test]
    fn html_file_becomes_one_document_node() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::create_dir_all(ws.join("pages")).unwrap();
        fs::write(
            ws.join("pages/index.html"),
            "<!doctype html>\n<html><body><div id=\"root\">hi</div></body></html>\n",
        )
        .unwrap();

        let (rt, writer) = writer_for(ws);
        let sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        let (counts, _pending) = ingest_html(&cfg(), ws, sink).expect("ingest");
        assert_eq!(counts.files, 1);
        // the document node + the `root` html_id node.
        assert_eq!(counts.symbols, 2);
        assert!(counts.elements >= 3, "html/body/div parsed");

        let reader = rt
            .block_on(kenn_store::reader_from_writer(&writer))
            .expect("reader");
        let doc = rt
            .block_on(Reader::fetch_symbol(
                &reader,
                "html",
                "html:pages/index.html",
            ))
            .expect("fetch")
            .expect("document node");
        assert_eq!(doc.kind, "document");
        assert_eq!(doc.pub_id, "html:pages/index.html");
    }

    /// A `.htm` file is indexed the same as `.html` (task spec scenario).
    #[test]
    fn htm_extension_is_indexed() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::write(ws.join("legacy.htm"), "<html><body>x</body></html>\n").unwrap();

        let (rt, writer) = writer_for(ws);
        let sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        let (counts, _pending) = ingest_html(&cfg(), ws, sink).expect("ingest");
        assert_eq!(counts.files, 1);

        let reader = rt
            .block_on(kenn_store::reader_from_writer(&writer))
            .expect("reader");
        assert!(rt
            .block_on(Reader::fetch_symbol(&reader, "html", "html:legacy.htm"))
            .expect("fetch")
            .is_some());
    }

    /// End-to-end producer + barrier through the real store: an HTML page links
    /// another doc, imports a CSS file, defines an `html_id` that corresponds to a
    /// `css_id`, and uses a registered class. Asserts each edge resolves (task
    /// 6.2/6.4 at the module level; the pipeline e2e lives in `pipeline/tests`).
    #[expect(
        clippy::too_many_lines,
        reason = "one linear barrier-integration fixture: seed nodes, run resolution, assert each edge kind in sequence; splitting would scatter the scenario"
    )]
    #[test]
    fn barrier_resolves_links_imports_correspondence_and_usage() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::write(
            ws.join("index.html"),
            "<!doctype html>\n\
             <link rel=\"stylesheet\" href=\"app.css\">\n\
             <a href=\"about.html\">about</a>\n\
             <div id=\"hero\" class=\"btn\">hi</div>\n",
        )
        .unwrap();
        fs::write(ws.join("about.html"), "<html><body>about</body></html>\n").unwrap();
        // A CSS file providing the `.btn` class registry node and the `#hero` id.
        fs::write(
            ws.join("app.css"),
            ".btn { color: red }\n#hero { top: 0 }\n",
        )
        .unwrap();

        let (rt, writer) = writer_for(ws);

        // CSS producer: registers `.btn` + `#hero`.
        let css_cfg = kenn_config::CssConfig {
            enabled: true,
            roots: vec![".".into()],
            ..Default::default()
        };
        let css_sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        crate::css::ingest_css_phase1(&css_cfg, ws, css_sink).expect("css phase1");

        // HTML producer.
        let html_sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        let (_c, pending) = ingest_html(&cfg(), ws, html_sink).expect("ingest html");

        // Barrier.
        let reader = rt
            .block_on(kenn_store::reader_from_writer(&writer))
            .expect("reader");
        let bsink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        let rc = resolve_html(pending, &reader, rt.handle(), ws, bsink).expect("resolve");
        assert!(
            rc.edges >= 4,
            "links+imports+correspondence+usage, got {}",
            rc.edges
        );
        drop(reader);

        let reader = rt
            .block_on(kenn_store::reader_from_writer(&writer))
            .expect("reader");
        // import edge: index.html → app.css
        let appcss = rt
            .block_on(Reader::fetch_symbol(&reader, "css", "css:app.css"))
            .expect("fetch")
            .expect("app.css module");
        let (imp, imp_total) = rt
            .block_on(Reader::list_inbound(
                &reader, appcss.id, "imports", 50, None, false, true,
            ))
            .expect("imports");
        assert_eq!(imp_total, 1);
        assert_eq!(imp[0].pub_id, "html:index.html");
        // links_to_file edge: index.html → about.html
        let about = rt
            .block_on(Reader::fetch_symbol(&reader, "html", "html:about.html"))
            .expect("fetch")
            .expect("about doc");
        let (_lnk, lnk_total) = rt
            .block_on(Reader::list_inbound(
                &reader,
                about.id,
                "links_to_file",
                50,
                None,
                false,
                true,
            ))
            .expect("links_to_file");
        assert_eq!(lnk_total, 1);
        // correspondence edge: html_id `hero` ↔ css_id `hero`
        let hero_html = rt
            .block_on(Reader::fetch_symbol(
                &reader,
                "html",
                "html:index.html#id:hero",
            ))
            .expect("fetch")
            .expect("hero html_id");
        let (_cor, cor_total) = rt
            .block_on(Reader::list_outbound(
                &reader,
                hero_html.id,
                "corresponds_to",
                50,
                None,
                false,
                true,
            ))
            .expect("corresponds_to");
        assert_eq!(cor_total, 1);
        // usage edge: the `hero` html_id uses `.btn` (enclosing-id attribution)
        let btn = rt
            .block_on(reader.symbols_by_short_name("btn"))
            .expect("btn")
            .into_iter()
            .find(|h| h.qualified == "css:app.css#class:btn")
            .expect("btn class node")
            .id;
        let (use_rows, use_total) = rt
            .block_on(Reader::list_inbound(
                &reader,
                btn,
                "uses_css_class",
                50,
                None,
                false,
                true,
            ))
            .expect("uses_css_class");
        assert_eq!(use_total, 1);
        assert_eq!(use_rows[0].pub_id, "html:index.html#id:hero");
    }

    /// Task 6.3: a class used ONLY in HTML is no longer reported dead by
    /// `check_css`. The HTML parser's `uses_css_class` edge gives `.only-html` a
    /// usage, so the orphan-class scan does not flag it.
    #[test]
    fn class_used_only_in_html_is_not_dead() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::write(ws.join("app.css"), ".only-html { color: red }\n").unwrap();
        fs::write(
            ws.join("page.html"),
            "<!doctype html>\n<div class=\"only-html\">hi</div>\n",
        )
        .unwrap();

        let (rt, writer) = writer_for(ws);
        let css_cfg = kenn_config::CssConfig {
            enabled: true,
            roots: vec![".".into()],
            ..Default::default()
        };
        let css_sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        crate::css::ingest_css_phase1(&css_cfg, ws, css_sink).expect("css phase1");

        let html_sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        let (_c, pending) = ingest_html(&cfg(), ws, html_sink).expect("ingest html");

        let reader = rt
            .block_on(kenn_store::reader_from_writer(&writer))
            .expect("reader");
        let bsink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
        resolve_html(pending, &reader, rt.handle(), ws, bsink).expect("resolve");
        drop(reader);

        let reader = rt
            .block_on(kenn_store::reader_from_writer(&writer))
            .expect("reader");
        let (rows, counts) = rt
            .block_on(reader.scan_css_health(true, false, 50))
            .expect("scan");
        assert!(counts.usage_mining_on, "an HTML usage edge turns mining on");
        assert_eq!(
            counts.orphan_classes, 0,
            "`.only-html` has an HTML usage, so it is not orphaned"
        );
        assert!(rows
            .iter()
            .all(|r| r.pub_id != "css:app.css#class:only-html"));
    }
}
