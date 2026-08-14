use std::collections::{HashMap, HashSet};

use kenn_model::{
    compose_short_id, EdgeProperties, EdgeRecord, ImportKind, Kind, Language, LinkGrade, ShortId,
    SymbolRecord,
};

use super::super::ids::HtmlIdIndex;
use super::super::parse::{Attr, Element};
use crate::markdown::{resolve_file_ref, CodeCandidate, CodeLookup};
use crate::relpath::{join_relative, PathExists};

/// The workspace file set HTML references resolve against — the "resolution
/// index of known files/documents" (relpath → file node id). Implements
/// [`CodeLookup`] so the markdown file resolver can be reused verbatim; HTML
/// references are paths, never bare symbols, so [`Self::symbols_by_short_name`]
/// is always empty.
#[derive(Debug, Default)]
pub struct WorkspaceFiles {
    files: Vec<(String, ShortId)>,
}

impl WorkspaceFiles {
    /// Build from `(workspace-relative path, file node id)` pairs.
    pub fn new(files: impl IntoIterator<Item = (String, ShortId)>) -> Self {
        Self {
            files: files.into_iter().collect(),
        }
    }
}

impl CodeLookup for WorkspaceFiles {
    fn files_by_basename(&self, basename: &str) -> Vec<CodeCandidate> {
        self.files
            .iter()
            .filter(|(rel, _)| rel.rsplit('/').next() == Some(basename))
            .map(|(rel, id)| CodeCandidate {
                id: *id,
                relpath: rel.clone(),
                qualified: rel.clone(),
            })
            .collect()
    }

    fn symbols_by_short_name(&self, _name: &str) -> Vec<CodeCandidate> {
        Vec::new()
    }
}

/// Allocator for HTML stub node ids, in the `html` short-id space. Continues
/// from `start` so dangling-stub ids never collide with the file/document ids
/// the keystone already minted.
#[derive(Debug)]
pub struct HtmlIds {
    next: u32,
}

impl HtmlIds {
    #[must_use]
    pub fn new(start: u32) -> Self {
        Self { next: start.max(1) }
    }

    /// The next id this allocator would mint — the high-water mark to seed a
    /// later allocator with so its ids continue without colliding (the barrier
    /// pass resumes stub minting from where the keystone's node minting left off).
    #[must_use]
    pub fn current(&self) -> u32 {
        self.next
    }

    /// Mint the next id in the `html` short-id space. `pub(in super::super)` so the
    /// `html_id`-node pass ([`super::super::ids`]) shares this allocator with stub
    /// minting — one monotonic counter per file keeps node and stub ids disjoint.
    pub(in super::super) fn mint(&mut self) -> ShortId {
        let id = compose_short_id(Language::Html, self.next);
        self.next += 1;
        id
    }
}

/// The per-file `html_id` anchor sets a fragment href resolves against — relpath
/// → that file's [`HtmlIdIndex`] (built by [`super::super::ids::html_id_nodes`]). The
/// HTML analog of markdown's section anchors; the caller assembles it across the
/// corpus (Phase 4), tests construct it inline.
#[derive(Debug, Default)]
pub struct FragmentIndex {
    by_relpath: HashMap<String, HtmlIdIndex>,
}

impl FragmentIndex {
    /// Build from `(relpath, that file's html_id index)` pairs.
    pub fn new(files: impl IntoIterator<Item = (String, HtmlIdIndex)>) -> Self {
        Self {
            by_relpath: files.into_iter().collect(),
        }
    }

    /// The `html_id` node named `frag` in file `relpath`, if both exist.
    fn get(&self, relpath: &str, frag: &str) -> Option<ShortId> {
        self.by_relpath.get(relpath)?.get(frag)
    }
}

/// Accumulates the dangling external stubs minted while resolving one file's
/// references, deduped by `pub_id`.
#[derive(Debug, Default)]
pub struct StubSink {
    by_pub_id: HashMap<String, ShortId>,
    pub records: Vec<SymbolRecord>,
}

impl StubSink {
    fn intern(&mut self, pub_id: &str, ids: &mut HtmlIds) -> ShortId {
        self.intern_with(pub_id, ids, stub_symbol)
    }

    /// Intern a stub whose target is known to exist — always an `attachment`
    /// (see [`attachment_symbol`]).
    fn intern_attachment(&mut self, pub_id: &str, ids: &mut HtmlIds) -> ShortId {
        self.intern_with(pub_id, ids, attachment_symbol)
    }

    fn intern_with(
        &mut self,
        pub_id: &str,
        ids: &mut HtmlIds,
        record: fn(ShortId, &str) -> SymbolRecord,
    ) -> ShortId {
        *self.by_pub_id.entry(pub_id.to_string()).or_insert_with(|| {
            let id = ids.mint();
            self.records.push(record(id, pub_id));
            id
        })
    }
}

/// `<a href>` link edges for one file (tasks 3.1, 4.2, 4.4). Each href runs the
/// design-D7 ladder: fragment → `html_id` anchor, indexed file → `LinksToFile`,
/// existing non-indexed asset → path-keyed `attachment` stub, else a dangling
/// `LinksTo`. Pure over its inputs; `doc_sym` is the file's document node (the
/// link source).
#[expect(
    clippy::too_many_arguments,
    reason = "pure resolver threads the three caller-built lookups plus the id allocator and stub sink"
)]
pub fn anchor_link_edges(
    elements: &[Element],
    linking_relpath: &str,
    doc_sym: ShortId,
    files: &dyn CodeLookup,
    frags: &FragmentIndex,
    assets: &dyn PathExists,
    ids: &mut HtmlIds,
    stubs: &mut StubSink,
) -> Vec<EdgeRecord> {
    let mut anchor = Anchor {
        files,
        frags,
        assets,
        ids,
        stubs,
        doc_sym,
        linking_relpath,
        seen: HashSet::new(),
        edges: Vec::new(),
    };
    for href in elements
        .iter()
        .filter(|e| e.tag == "a")
        .filter_map(|e| attr(e, "href"))
    {
        anchor.resolve(href);
    }
    anchor.edges
}

/// `<img>`/`<video>`/`<source>`/`<iframe>` `src` asset edges for one file (task
/// 4.4). Each becomes a `LinksTo` to an `attachment` **stub** in the symbol space
/// (design D7) keyed by the **canonical workspace-relative path**, so every
/// spelling of one on-disk asset (`logo.png`, `../logo.png`, `/assets/logo.png`)
/// collapses to a single node — making reverse lookup deterministic. A `src`
/// whose target does not exist on disk dangles by its written string.
pub fn asset_link_edges(
    elements: &[Element],
    linking_relpath: &str,
    doc_sym: ShortId,
    assets: &dyn PathExists,
    ids: &mut HtmlIds,
    stubs: &mut StubSink,
) -> Vec<EdgeRecord> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(ShortId, ShortId)> = HashSet::new();
    for src in media_srcs(elements) {
        if is_external_url(src) {
            continue;
        }
        let (stub, grade) = mint_asset(src, linking_relpath, assets, ids, stubs);
        if push_unique(&mut seen, doc_sym, stub) {
            edges.push(links_to(doc_sym, stub, grade));
        }
    }
    edges
}

/// Mutable state for resolving one file's `<a href>` references: the caller's
/// lookups, the shared id allocator + stub sink, and this file's dedup set +
/// output. Bundling them keeps each ladder branch a small `&mut self` method
/// rather than an 8-argument free function.
struct Anchor<'a> {
    files: &'a dyn CodeLookup,
    frags: &'a FragmentIndex,
    assets: &'a dyn PathExists,
    ids: &'a mut HtmlIds,
    stubs: &'a mut StubSink,
    doc_sym: ShortId,
    linking_relpath: &'a str,
    seen: HashSet<(ShortId, ShortId)>,
    edges: Vec<EdgeRecord>,
}

impl Anchor<'_> {
    /// Run one `<a href>` through the design-D7 ladder.
    fn resolve(&mut self, href: &str) {
        if is_external_url(href) {
            return;
        }
        if let Some(frag) = fragment_of(href) {
            self.fragment(href, frag);
            return;
        }
        let file_part = href.trim();
        if file_part.is_empty() {
            return;
        }
        let targets = resolve_file_ref(file_part, self.linking_relpath, self.files);
        if targets.is_empty() {
            // Existence decides, not spelling. `mint_asset` already dangles a
            // target the workspace does not hold, so the old `is_asset_ref`
            // gate only ever suppressed *existing* targets whose extension kenn
            // indexes — an excluded `.md` dangled here while markdown resolved
            // it, which is the divergence this change removes.
            let (stub, grade) = mint_asset(
                file_part,
                self.linking_relpath,
                self.assets,
                self.ids,
                self.stubs,
            );
            self.push(stub, links_props(grade));
        } else {
            for t in targets {
                self.push(t.id, EdgeProperties::LinksToFile { grade: t.grade });
            }
        }
    }

    /// A fragment href (`#frag` / `page#frag`) → `LinksTo` the target file's
    /// `html_id` anchor; an unknown fragment dangles by the written href.
    fn fragment(&mut self, href: &str, frag: &str) {
        let file_part = href.split('#').next().unwrap_or("").trim();
        // `None` = the href walks above the workspace root, so there is no
        // in-corpus file to look the fragment up in; fall through to dangling
        // rather than resolving it against the root.
        let target_rel = if file_part.is_empty() {
            Some(self.linking_relpath.to_string())
        } else {
            join_relative(self.linking_relpath, file_part)
        };
        if let Some(id) = target_rel.and_then(|rel| self.frags.get(&rel, frag)) {
            self.push(id, links_props(LinkGrade::Exact));
        } else {
            let stub = self.stubs.intern(&unresolved_id(href), self.ids);
            self.push(stub, links_props(LinkGrade::Dangling));
        }
    }

    /// Emit `doc_sym --props--> target`, deduped on `(src, target)`.
    fn push(&mut self, target: ShortId, properties: EdgeProperties) {
        if self.seen.insert((self.doc_sym, target)) {
            self.edges.push(EdgeRecord {
                src_id: self.doc_sym,
                target_id: target,
                properties,
            });
        }
    }
}

/// Intern the `attachment` stub for an asset reference: an existing asset keys by
/// its canonical workspace-relative path (`html:<canonical>`, grade `Exact`); a
/// missing one keys by the written string (`html:@unresolved/<written>`, grade
/// `Dangling`). Returns the stub id and the edge grade.
fn mint_asset(
    href: &str,
    linking_relpath: &str,
    assets: &dyn PathExists,
    ids: &mut HtmlIds,
    stubs: &mut StubSink,
) -> (ShortId, LinkGrade) {
    // An href walking above the workspace root has no canonical in-corpus path,
    // so it cannot key a shared stub — it dangles by its written string.
    let Some(canonical) = join_relative(linking_relpath, href) else {
        return (stubs.intern(&unresolved_id(href), ids), LinkGrade::Dangling);
    };
    if assets.exists(&canonical) {
        (
            stubs.intern_attachment(&format!("html:{}", crate::pubid::floor(&canonical)), ids),
            LinkGrade::Exact,
        )
    } else {
        (stubs.intern(&unresolved_id(href), ids), LinkGrade::Dangling)
    }
}

/// A `LinksTo` edge of the given grade (the symbol-targeting link kind, design
/// D7).
fn links_to(src: ShortId, target: ShortId, grade: LinkGrade) -> EdgeRecord {
    EdgeRecord {
        src_id: src,
        target_id: target,
        properties: links_props(grade),
    }
}

fn links_props(grade: LinkGrade) -> EdgeProperties {
    EdgeProperties::LinksTo {
        grade,
        relation: String::new(),
    }
}

/// The non-empty fragment of an href (`page#intro` → `intro`, `#top` → `top`),
/// or `None` when there is no `#` or an empty fragment (`page#` → a file ref).
fn fragment_of(href: &str) -> Option<&str> {
    let (_, frag) = href.split_once('#')?;
    (!frag.is_empty()).then_some(frag)
}

/// `<img>`/`<video>`/`<source>`/`<iframe>` `src` values in document order.
fn media_srcs(elements: &[Element]) -> Vec<&str> {
    elements
        .iter()
        .filter(|e| matches!(e.tag.as_str(), "img" | "video" | "source" | "iframe"))
        .filter_map(|e| attr(e, "src"))
        .collect()
}

/// Whether `ext` (any case) belongs to a producer kenn indexes — HTML, CSS/Sass,
/// markdown, or a code language. Everything else (png, svg, pdf, woff…) is an
/// asset. The set mirrors the discovery extensions of the indexed languages.
fn is_indexed_ext(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    matches!(
        e.as_str(),
        "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "md"
            | "markdown"
            | "rs"
            | "go"
            | "py"
            | "cs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "mts"
            | "cts"
    )
}

/// `<link rel="stylesheet" href>` and `<script src>` import edges for one file
/// (task 3.2). Each becomes an `Imports` edge from the document to the
/// referenced file (design D7); a target not in the workspace becomes a dangling
/// stub rather than being dropped. Pure over its inputs.
pub fn import_edges(
    elements: &[Element],
    linking_relpath: &str,
    doc_sym: ShortId,
    files: &dyn CodeLookup,
    ids: &mut HtmlIds,
    stubs: &mut StubSink,
) -> Vec<EdgeRecord> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(ShortId, ShortId)> = HashSet::new();
    for href in import_refs(elements) {
        let Some(file_part) = resolvable_path(href) else {
            continue;
        };
        for edge in resolve_ref(
            file_part,
            linking_relpath,
            doc_sym,
            files,
            ids,
            stubs,
            &mut seen,
            import_props,
        ) {
            edges.push(edge);
        }
    }
    edges
}

/// Resolve one reference path into edges, minting a dangling stub when it
/// resolves to nothing. `props` builds the edge for a resolved target (carrying
/// its grade); a dangling reference is always a `LinksTo`/`Dangling` to the
/// stub (symbol-space target, design D7). `seen` dedups `(src, target)` so a
/// page that names the same target twice yields one edge.
#[expect(
    clippy::too_many_arguments,
    reason = "pure resolver threads the file index, id allocator, stub sink, and dedup set"
)]
fn resolve_ref(
    file_part: &str,
    linking_relpath: &str,
    doc_sym: ShortId,
    files: &dyn CodeLookup,
    ids: &mut HtmlIds,
    stubs: &mut StubSink,
    seen: &mut HashSet<(ShortId, ShortId)>,
    props: fn(LinkGrade) -> EdgeProperties,
) -> Vec<EdgeRecord> {
    let targets = resolve_file_ref(file_part, linking_relpath, files);
    if targets.is_empty() {
        let stub = stubs.intern(&unresolved_id(file_part), ids);
        return if push_unique(seen, doc_sym, stub) {
            dangling_edge(doc_sym, stub, props)
        } else {
            Vec::new()
        };
    }
    targets
        .into_iter()
        .filter(|t| push_unique(seen, doc_sym, t.id))
        .map(|t| EdgeRecord {
            src_id: doc_sym,
            target_id: t.id,
            properties: props(t.grade),
        })
        .collect()
}

/// A resolved `<link>`/`<script>` target is an `Imports` edge.
fn import_props(_grade: LinkGrade) -> EdgeProperties {
    EdgeProperties::Imports {
        kind: ImportKind::Explicit,
    }
}

/// The edge to a dangling stub: a `Dangling` `LinksTo` for an `<a href>`; for an
/// import, the `Imports` edge to the stub (Imports carries no grade — the
/// `!unresolved` stub target is the danglingness).
fn dangling_edge(
    src: ShortId,
    stub: ShortId,
    props: fn(LinkGrade) -> EdgeProperties,
) -> Vec<EdgeRecord> {
    let properties = match props(LinkGrade::Dangling) {
        EdgeProperties::Imports { kind } => EdgeProperties::Imports { kind },
        _ => EdgeProperties::LinksTo {
            grade: LinkGrade::Dangling,
            relation: String::new(),
        },
    };
    vec![EdgeRecord {
        src_id: src,
        target_id: stub,
        properties,
    }]
}

/// `<link>`/`<script>` reference hrefs in document order: a `<link>` whose `rel`
/// names `stylesheet`, by its `href`; a `<script>` by its `src` (a `<script>`
/// without `src` is inline — Phase 3, skipped here).
fn import_refs(elements: &[Element]) -> Vec<&str> {
    let mut out = Vec::new();
    for e in elements {
        let href = match e.tag.as_str() {
            "link" if is_stylesheet(e) => attr(e, "href"),
            "script" => attr(e, "src"),
            _ => None,
        };
        if let Some(h) = href {
            out.push(h);
        }
    }
    out
}

/// The fragment-stripped, in-workspace path to resolve, or `None` to skip: an
/// external URL, an empty value, or a bare same-page `#frag` (empty file part —
/// the `html_id` anchor pass is Phase 2). A `page#frag` keeps `page`.
fn resolvable_path(href: &str) -> Option<&str> {
    if is_external_url(href) {
        return None;
    }
    let file_part = href.split('#').next().unwrap_or(href).trim();
    (!file_part.is_empty()).then_some(file_part)
}

/// True when `rel` (space-separated tokens, case-insensitive) names a
/// stylesheet, e.g. `rel="stylesheet"` or `rel="preload stylesheet"`.
fn is_stylesheet(e: &Element) -> bool {
    attr(e, "rel").is_some_and(|rel| {
        rel.split_whitespace()
            .any(|t| t.eq_ignore_ascii_case("stylesheet"))
    })
}

/// A reference that does not point inside the workspace: any scheme'd or
/// protocol-relative URL. A site-absolute `/path` is NOT external (it resolves
/// by basename against the file set).
fn is_external_url(href: &str) -> bool {
    let lower = href.trim_start().to_ascii_lowercase();
    href.contains("://")
        || lower.starts_with("//")
        || [
            "http:",
            "https:",
            "mailto:",
            "tel:",
            "ftp:",
            "ftps:",
            "data:",
            "javascript:",
        ]
        .iter()
        .any(|s| lower.starts_with(s))
}

/// First value of `e`'s named attribute (ASCII-folded name), if present.
fn attr<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
    e.attrs
        .iter()
        .find(|a: &&Attr| a.name == name)
        .map(|a| a.value.as_str())
}

fn push_unique(seen: &mut HashSet<(ShortId, ShortId)>, src: ShortId, target: ShortId) -> bool {
    seen.insert((src, target))
}

/// Stable, shell-safe `pub_id` for an unresolved HTML reference (the dangling
/// stub's id), mirroring markdown's `@unresolved` scheme under the `html:`
/// prefix. The `target` (raw href text) is floored through [`crate::pubid::floor`]
/// so the `pub_id` is a single shell token; `@` replaces the shell-hostile `!`
/// sentinel.
fn unresolved_id(target: &str) -> String {
    format!("html:@unresolved/{}", crate::pubid::floor(target))
}

/// An external stub for an unresolved reference (`html:@unresolved/…`) or an
/// existing-but-non-indexed asset (`html:<canonical>`). A non-indexed asset
/// extension with a known MIME (png/pdf/svg…) is an `attachment` leaf (design
/// D7); an indexed-but-missing target (`gone.html`, `theme.css`) stays a
/// `document` stub.
fn stub_symbol(id: ShortId, pub_id: &str) -> SymbolRecord {
    let name = stub_name(pub_id);
    let kind = stub_kind(&name);
    stub_record(id, pub_id, name, kind)
}

/// The display name behind a stub's `pub_id` — the written target for a
/// dangling stub, the canonical path for a resolved one.
fn stub_name(pub_id: &str) -> String {
    pub_id
        .strip_prefix("html:@unresolved/")
        .or_else(|| pub_id.strip_prefix("html:"))
        .unwrap_or(pub_id)
        .to_string()
}

fn stub_record(id: ShortId, pub_id: &str, name: String, kind: Kind) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.to_string(),
        language: Language::Html,
        pkg_id: 0,
        kind,
        name,
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: true,
        test: false,
    }
}

/// `Attachment` when `name` is a non-indexed asset with a known (non-markdown)
/// MIME — a leaf binary kenn does not index; `Document` otherwise (an
/// extensionless note or a missing indexed file). Mirrors markdown's
/// asset-vs-note split, extended to exclude HTML/CSS/JS indexed extensions.
fn stub_kind(name: &str) -> Kind {
    let ext = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .rsplit_once('.')
        .map_or("", |(_, e)| e);
    if !ext.is_empty() && !is_indexed_ext(ext) && mime_guess::from_path(name).first().is_some() {
        Kind::Attachment
    } else {
        Kind::Document
    }
}

/// The record for a stub whose target is **known to exist** in the workspace.
/// Kinded by [`crate::markdown::existing_target_kind`] — the same rule the
/// markdown corpus applies, so one on-disk target is not a leaf on one side and
/// a document on the other. [`stub_kind`]'s guess is for a *dangling* stub,
/// which has only a written string to go on; running it here and discarding the
/// result would be dead work on every asset reference.
fn attachment_symbol(id: ShortId, pub_id: &str) -> SymbolRecord {
    let name = stub_name(pub_id);
    let kind = crate::markdown::existing_target_kind(&name);
    stub_record(id, pub_id, name, kind)
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
