use std::collections::HashMap;

use kenn_config::MarkdownConfig;
use kenn_model::id::md::{document_id, module_chain, module_id};
use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, Kind, Language, LinkGrade, ShortId, SymbolRecord,
};
use kenn_store::api::DbError;
use kenn_store::DbReader;
use tokio::runtime::Handle;

use super::super::index::ResolutionIndex;
use super::super::links::{extract_links, LinkKind, RawLink};
use super::super::resolve::{dangling_id, dangling_name, resolve_link};
use super::super::walk::{walk_markdown, MarkdownIds};
use super::super::StoreCodeLookup;
use super::super::{
    collect, discover_markdown, resolve_code_link, CodeTarget, MarkdownDiscoverError,
};
pub use crate::relpath::FsPaths;
use crate::relpath::{join_relative, PathExists};
use crate::sink::BatchSink;

#[derive(Debug, thiserror::Error)]
pub enum MarkdownIngestError {
    #[error(transparent)]
    Discover(#[from] MarkdownDiscoverError),
    #[error("store append failed: {0}")]
    Store(#[from] DbError),
}

/// Per-file state retained between the walk pass and the link-resolution pass.
struct FileState {
    doc_id: String,
    doc_sym: ShortId,
    /// The file's relative path — locality context for md→code resolution.
    relpath: String,
    /// In-repo files defer unresolved links for an md→code attempt; external
    /// vault files dangle them immediately (design D6).
    in_repo: bool,
    defs: Vec<DefRecord>,
}

/// A link that failed md↔md resolution in an in-repo file, carried past the
/// code barrier for an md→code attempt (design D4/D6).
struct DeferredLink {
    src: ShortId,
    raw: RawLink,
    linking_relpath: String,
}

/// State handed from phase 1 to the post-code md→code resolution: the id
/// allocator (so stub ids continue without colliding with phase-1 nodes) and
/// the deferred in-repo links awaiting the code graph.
pub struct MarkdownPending {
    ids: MarkdownIds,
    deferred: Vec<DeferredLink>,
}

/// Record counts produced by a markdown ingest phase, for the run report (so
/// `kenn status` meta and the count-regression check see markdown, not zeros).
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownCounts {
    pub files: u64,
    pub symbols: u64,
    pub defs: u64,
    pub edges: u64,
}

/// Phase 1 (design D4): discover + collect + walk every file, emit nodes and
/// md↔md edges through `sink`, and finish it. Returns the file count and the
/// [`MarkdownPending`] to resolve after the code barrier.
pub fn ingest_markdown_phase1(
    config: &MarkdownConfig,
    workspace_root: &std::path::Path,
    mut sink: BatchSink,
) -> Result<(MarkdownCounts, MarkdownPending), MarkdownIngestError> {
    let mut counts = MarkdownCounts::default();
    // Pass 1 — read + collect every file.
    let mut files = Vec::new();
    for file in discover_markdown(config, workspace_root)? {
        let Ok(content) = std::fs::read_to_string(&file.abs_path) else {
            tracing::warn!(
                target: "kenn_indexer::markdown",
                path = %file.abs_path.display(),
                "unreadable markdown file, skipped"
            );
            continue;
        };
        let collected = collect(&content);
        files.push((file, content, collected));
    }
    let index = ResolutionIndex::build(files.iter().map(|(f, _, c)| (f, c)));

    // Pass 2a — build the corpus module tree (root + nested directory modules,
    // deduped across files) so each document has an enclosing module to belong
    // to. Minted before the walk so module ids precede file/node ids.
    let mut ids = MarkdownIds::new();
    let (module_records, module_edges, immediate_modules) = build_modules(&files, &mut ids);
    counts.symbols += module_records.len() as u64;
    counts.edges += module_edges.len() as u64;
    for m in module_records {
        sink.push_symbol(m)?;
    }
    for e in module_edges {
        sink.push_edge(e)?;
    }

    // Pass 2b — walk every file: emit nodes, build pub_id → ShortId, stash state.
    let mut node_ids: HashMap<String, ShortId> = HashMap::new();
    let mut states: Vec<FileState> = Vec::new();
    for ((file, content, collected), &module) in files.iter().zip(immediate_modules.iter()) {
        let records = walk_markdown(file, content, collected, &mut ids, module);
        let doc_id = document_id(&file.label, &file.relpath).into_string();
        let doc_sym = records.symbols.first().map_or(0, |s| s.id);
        for s in &records.symbols {
            node_ids.insert(s.pub_id.clone(), s.id);
        }
        states.push(FileState {
            doc_id,
            doc_sym,
            relpath: file.relpath.clone(),
            in_repo: file.in_repo,
            defs: records.defs.clone(),
        });
        counts.symbols += records.symbols.len() as u64;
        counts.defs += records.defs.len() as u64;
        counts.edges += records.edges.len() as u64;
        sink.push_document_records(
            std::iter::once(records.file),
            records.symbols,
            records.docs,
            records.defs,
            records.edges,
        )?;
    }
    counts.files = files.len() as u64;

    // Pass 3 — resolve md↔md links → edges; defer in-repo dangling, stub the
    // rest.
    let mut stubs: HashMap<String, ShortId> = HashMap::new();
    let mut stub_records: Vec<SymbolRecord> = Vec::new();
    let mut deferred: Vec<DeferredLink> = Vec::new();
    for ((_, content, _), state) in files.iter().zip(states.iter()) {
        let edges = file_link_edges(
            content,
            state,
            &node_ids,
            &index,
            &mut ids,
            &mut stubs,
            &mut stub_records,
            &mut deferred,
        );
        counts.edges += edges.len() as u64;
        for edge in edges {
            sink.push_edge(edge)?;
        }
    }
    counts.symbols += stub_records.len() as u64;
    for stub in stub_records {
        sink.push_symbol(stub)?;
    }

    sink.finish()?;
    Ok((counts, MarkdownPending { ids, deferred }))
}

/// Post-code barrier (design D4/D6): resolve each deferred in-repo link against
/// the code graph and emit md→code edges; links that still don't match become
/// dangling stubs. `code` is the building snapshot + its runtime handle, or
/// `None` for a code-less run (every deferred link dangles). Owns and finishes
/// `sink`.
pub fn resolve_markdown_code(
    pending: MarkdownPending,
    code: Option<(&DbReader, &Handle)>,
    exists: &dyn PathExists,
    mut sink: BatchSink,
) -> Result<MarkdownCounts, MarkdownIngestError> {
    let MarkdownPending { mut ids, deferred } = pending;
    let mut counts = MarkdownCounts::default();

    // Phase A — resolve all (reads only). Resolving every link before any write
    // lets the read snapshot's statements finalize before the sink writes to
    // the same building store.
    let resolved: Vec<(ShortId, RawLink, String, Vec<CodeTarget>)> = deferred
        .into_iter()
        .map(|d| {
            // An inline CommonMark destination *means* a path — `[t](docs)` in
            // a README names the directory, not a `fn docs` that happens to
            // share the name. So when the target is a bare name that the
            // workspace holds as a path, skip the symbol lookup that would
            // otherwise shadow it. A wikilink is the opposite convention: a
            // bare `[[OrderHandler]]` is a name, so it keeps symbol-first.
            // Only a *bare name* can be shadowed: a path-shaped target
            // (`src/order.rs`) goes down the file branch, which resolves it to
            // its indexed file node — that must still win over an attachment.
            let path_wins = !d.raw.wikilink
                && !super::super::is_code_path(&d.raw.target)
                && attachment_key(&d.raw, &d.linking_relpath, exists).is_some();
            let targets = match code {
                Some((reader, handle)) if !path_wins => {
                    let lookup = StoreCodeLookup { reader, handle };
                    resolve_code_link(&d.raw.target, &d.linking_relpath, &lookup)
                }
                _ => Vec::new(),
            };
            (d.src, d.raw, d.linking_relpath, targets)
        })
        .collect();

    // Phase B — emit md→code edges, dangling-stub the rest.
    let mut stubs: HashMap<String, ShortId> = HashMap::new();
    let mut stub_records: Vec<SymbolRecord> = Vec::new();
    for (src, raw, linking_relpath, targets) in &resolved {
        if targets.is_empty() {
            // The target matched no markdown document, code file, or code
            // symbol — but it may still be a real file or directory kenn does
            // not index (`LICENSE-MIT`, `docs/`). That is not a broken link, so
            // it resolves to a path-keyed `attachment` stub, exactly as HTML
            // already does for `<img src="logo.png">`. Only a target the
            // workspace does not hold dangles.
            let (id, grade) = match attachment_key(raw, linking_relpath, exists) {
                Some(key) => (
                    mint_attachment(&key, &mut ids, &mut stubs, &mut stub_records),
                    attachment_grade(raw),
                ),
                None => (
                    mint_stub(raw, &mut ids, &mut stubs, &mut stub_records),
                    LinkGrade::Dangling,
                ),
            };
            sink.push_edge(link_edge(*src, id, raw, grade))?;
            counts.edges += 1;
        } else {
            for t in targets {
                // A code FILE target gets a `links_to_file` edge (hydrated from
                // the files table); a code symbol or md node gets `links_to`/
                // `embeds` by the source link's kind.
                let edge = if t.is_file {
                    EdgeRecord {
                        src_id: *src,
                        target_id: t.id,
                        properties: EdgeProperties::LinksToFile { grade: t.grade },
                    }
                } else {
                    link_edge(*src, t.id, raw, t.grade)
                };
                sink.push_edge(edge)?;
                counts.edges += 1;
            }
        }
    }
    counts.symbols += stub_records.len() as u64;
    for stub in stub_records {
        sink.push_symbol(stub)?;
    }

    sink.finish()?;
    Ok(counts)
}

/// Resolve every link in one file into md↔md `links_to`/`embeds` edges. Pure
/// over its inputs (no I/O / no sink) so it is unit-testable. A link that fails
/// md resolution is deferred (in-repo, pushed to `deferred`) or dangled now
/// (external vault, minting through `ids`/`stubs`/`stub_records`).
#[expect(
    clippy::too_many_arguments,
    reason = "the pure resolver threads the id allocator + stub/defer accumulators"
)]
fn file_link_edges(
    content: &str,
    state: &FileState,
    node_ids: &HashMap<String, ShortId>,
    index: &ResolutionIndex,
    ids: &mut MarkdownIds,
    stubs: &mut HashMap<String, ShortId>,
    stub_records: &mut Vec<SymbolRecord>,
    deferred: &mut Vec<DeferredLink>,
) -> Vec<EdgeRecord> {
    let mut edges = Vec::new();
    for raw in extract_links(content) {
        let src = enclosing_section(&state.defs, raw.line, state.doc_sym);
        for target in resolve_link(&raw, &state.doc_id, &state.relpath, index) {
            if target.external_stub {
                // md↔md failed. In-repo: defer for an md→code attempt past the
                // barrier. External vault: dangle now (no code resolution, D6).
                if state.in_repo {
                    deferred.push(DeferredLink {
                        src,
                        raw: raw.clone(),
                        linking_relpath: state.relpath.clone(),
                    });
                } else {
                    // target.node_id == dangling_id(&raw) for an external stub;
                    // pass the raw link so mint_stub keeps the id + name in sync.
                    let id = mint_stub(&raw, ids, stubs, stub_records);
                    edges.push(link_edge(src, id, &raw, target.grade));
                }
            } else if let Some(id) = node_ids.get(&target.node_id) {
                edges.push(link_edge(src, *id, &raw, target.grade));
            }
            // else: target node not in corpus (shouldn't happen) — skip.
        }
    }
    edges
}

/// Build a `links_to`/`embeds` edge from one resolved target, by link kind.
fn link_edge(src: ShortId, target_id: ShortId, raw: &RawLink, grade: LinkGrade) -> EdgeRecord {
    let properties = match raw.kind {
        LinkKind::Link => EdgeProperties::LinksTo {
            grade,
            relation: String::new(),
        },
        LinkKind::Embed => EdgeProperties::Embeds { grade },
    };
    EdgeRecord {
        src_id: src,
        target_id,
        properties,
    }
}

/// Build the corpus module tree: a `Kind::Module` node per root and per nested
/// directory, deduped across all files, chained `child --defined_in--> parent`.
/// Returns the module records, the inter-module `defined_in` edges, and — in
/// `files` order — each file's immediate (directory) module id, so the walk pass
/// can enclose the document without a fallible map lookup. Modules carry no def
/// — they are directory containers, not source spans.
fn build_modules(
    files: &[(
        super::super::DiscoveredMarkdown,
        String,
        super::super::CollectedFile,
    )],
    ids: &mut MarkdownIds,
) -> (Vec<SymbolRecord>, Vec<EdgeRecord>, Vec<ShortId>) {
    let mut seen: HashMap<String, ShortId> = HashMap::new();
    let mut records = Vec::new();
    let mut edges = Vec::new();
    let mut immediate = Vec::with_capacity(files.len());
    for (file, _, _) in files {
        let mut parent: ShortId = 0;
        for dir in module_chain(&file.relpath) {
            let pub_id = module_id(&file.label, &dir).into_string();
            parent = if let Some(existing) = seen.get(&pub_id) {
                *existing
            } else {
                let id = ids.mint_symbol();
                seen.insert(pub_id.clone(), id);
                records.push(module_symbol(
                    id,
                    &pub_id,
                    module_name(&file.label, &dir),
                    parent,
                ));
                if parent != 0 {
                    edges.push(EdgeRecord {
                        src_id: id,
                        target_id: parent,
                        properties: EdgeProperties::DefinedIn,
                    });
                }
                id
            };
        }
        // `parent` now holds the file's immediate (deepest) directory module.
        immediate.push(parent);
    }
    (records, edges, immediate)
}

/// Display name of a module: the root `label` for the root module, else the last
/// path segment of its directory.
fn module_name(label: &str, dir: &str) -> String {
    if dir.is_empty() {
        label.to_string()
    } else {
        dir.rsplit('/').next().unwrap_or(dir).to_string()
    }
}

fn module_symbol(id: ShortId, pub_id: &str, name: String, enclosing: ShortId) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: crate::pubid::floor(pub_id),
        language: Language::Markdown,
        pkg_id: 0,
        kind: Kind::Module,
        name,
        enclosing_sym_id: enclosing,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

/// The grade for a resolved attachment. `Exact` when the written target is the
/// whole claim, `Drifted` when it carries an anchor: an attachment is not in the
/// corpus, so its sections are unknown and the anchor cannot be verified.
/// `apply_anchor` sets the precedent for md↔md — anchor present but unmatched →
/// target the document, at least Drifted — and grading
/// `[notes](vendor/CHANGELOG.md#v1-0-0)` exact would assert a section this pass
/// never looked at.
fn attachment_grade(raw: &super::super::links::RawLink) -> LinkGrade {
    if raw.anchor.is_some() {
        LinkGrade::Drifted
    } else {
        LinkGrade::Exact
    }
}

/// The canonical workspace-relative path a link target resolves to when the
/// workspace holds it, or `None` when it does not — the attachment rung of the
/// ladder (design D4).
///
/// The written target is joined onto the linking file's directory, so every
/// spelling of one on-disk target (`LICENSE-MIT` from the root,
/// `../../LICENSE-MIT` from a crate doc) produces the same key and therefore the
/// same node. A target walking above the workspace root has no in-corpus
/// canonical path and is not an attachment.
fn attachment_key(
    raw: &super::super::links::RawLink,
    linking_relpath: &str,
    exists: &dyn PathExists,
) -> Option<String> {
    // `RawLink::target` is anchor-free by contract (the `#anchor` rides in
    // `raw.anchor`), so there is nothing to strip here.
    let written = raw.target.trim().trim_start_matches("./");
    if written.is_empty() {
        return None;
    }
    // The same two spellings the exact rung accepts (see `resolve_file_ref`):
    // the path as written, already workspace-relative, then the path joined
    // onto the linking file's directory. The rungs MUST agree on what a written
    // target means — a target that the graph would have matched as written,
    // but that this rung only tried joined, would dangle for a reason no reader
    // could infer.
    // Both spellings go through the same join so the KEY is canonical either
    // way. Joining against an empty linking path just normalizes (drops `./`,
    // empty segments, and a trailing `/`), which is what makes `[a](docs/)` and
    // `[b](docs)` collapse to one node instead of two — the property
    // `list_usages` on an attachment depends on.
    //
    // The **joined** spelling is tried first, and that ordering matters here in
    // a way it does not for `resolve_file_ref`. There the as-written probe is
    // checked against a basename-filtered candidate set drawn from the graph;
    // here it is checked against the whole filesystem, where `docs`, `src`,
    // `tests` and `assets` routinely exist at the root *and* nested. Probing
    // root-first would bind `[the docs](docs)` written in
    // `crates/kenn-indexer/README.md` to the repository-root `docs/` — a
    // directory the link does not name, which is the false-match class this
    // change exists to remove. Relative wins; root-relative is the fallback.
    let hit = |c: &String| !c.is_empty() && exists.exists(c);
    let joined = join_relative(linking_relpath, written);
    if joined.as_ref().is_some_and(&hit) {
        return joined;
    }
    let root = join_relative("", written)?;
    // A root-level linking file makes both spellings identical; skip the second
    // stat rather than asking the filesystem the same question twice.
    (joined.as_deref() != Some(root.as_str()) && hit(&root)).then_some(root)
}

/// Intern the `attachment` stub for a target that exists in the workspace but is
/// not an indexed node, keyed by its canonical path so every reference to one
/// on-disk target collapses to a single node (the property `list_usages` needs).
fn mint_attachment(
    canonical: &str,
    ids: &mut MarkdownIds,
    stubs: &mut HashMap<String, ShortId>,
    stub_records: &mut Vec<SymbolRecord>,
) -> ShortId {
    let pub_id = format!("md:@attachment/{}", crate::pubid::floor(canonical));
    *stubs.entry(pub_id.clone()).or_insert_with(|| {
        let id = ids.mint_symbol();
        stub_records.push(attachment_stub(id, &pub_id, canonical.to_string()));
        id
    })
}

/// Intern a dangling target's external-stub node id, minting the stub record
/// on first sight (deduped by `pub_id`).
fn mint_stub(
    raw: &super::super::links::RawLink,
    ids: &mut MarkdownIds,
    stubs: &mut HashMap<String, ShortId>,
    stub_records: &mut Vec<SymbolRecord>,
) -> ShortId {
    let pub_id = dangling_id(raw);
    *stubs.entry(pub_id.clone()).or_insert_with(|| {
        let id = ids.mint_symbol();
        stub_records.push(external_stub(id, &pub_id, dangling_name(raw)));
        id
    })
}

/// The smallest section range covering `line`, or `fallback` (the document)
/// when no section does. The document's own file-spanning def is excluded so a
/// top section that spans the whole file still wins over the document.
fn enclosing_section(defs: &[DefRecord], line: u32, fallback: ShortId) -> ShortId {
    defs.iter()
        .filter(|d| d.sym_id != fallback)
        .filter(|d| d.start_line <= line && line <= d.end_line)
        .min_by_key(|d| d.end_line.saturating_sub(d.start_line))
        .map_or(fallback, |d| d.sym_id)
}

/// MIME type of an unresolved target by file extension, or `None` when it
/// has no extension or maps to markdown — i.e. it is a note, not an asset.
/// `![[diagram.png]]` → `image/png`; `[[Some Note]]` / `[[note.md]]` → `None`.
fn attachment_mime(name: &str) -> Option<mime_guess::Mime> {
    let mime = mime_guess::from_path(name).first()?;
    (mime.essence_str() != "text/markdown").then_some(mime)
}

/// The node kind for a target that **exists** but resolved to no graph node.
///
/// `Document` when the name is something navigable kenn would have indexed as a
/// document — a `.md` or `.html` the config excluded — and `Attachment`
/// otherwise, covering binaries, extensionless files like `LICENSE-MIT`, and
/// directories. Unlike [`external_stub`]'s guess, which works from a written
/// string that may name nothing, here the target is known to exist; the only
/// open question is whether it is a leaf.
///
/// Shared with the HTML corpus (`html-index` describes its asset handling as
/// "reusing the markdown attachment model") so one on-disk target is never a
/// leaf on one side and a document on the other.
#[must_use]
pub fn existing_target_kind(name: &str) -> Kind {
    let navigable = mime_guess::from_path(name)
        .first()
        .is_some_and(|m| matches!(m.essence_str(), "text/markdown" | "text/html"));
    if navigable {
        Kind::Document
    } else {
        Kind::Attachment
    }
}

/// The record for a resolved attachment: a stub standing for a real file or
/// directory kenn does not index, kinded by [`existing_target_kind`].
fn attachment_stub(id: ShortId, pub_id: &str, name: String) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.to_string(),
        language: Language::Markdown,
        pkg_id: 0,
        kind: existing_target_kind(&name),
        name,
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        // `external` marks a node with no workspace *definition*, which an
        // attachment never has (kenn indexes no defs for a licence or a
        // directory). Matches the flags HTML's asset stubs already carry.
        external: true,
        test: false,
    }
}

fn external_stub(id: ShortId, pub_id: &str, name: String) -> SymbolRecord {
    // An unresolved target with a non-markdown MIME (png/pdf/css/…) is an
    // attachment, not a note: a leaf stub whose type is its extension's MIME.
    // The MIME guess reads the raw (unescaped) name so its extension survives.
    let kind = if attachment_mime(&name).is_some() {
        Kind::Attachment
    } else {
        Kind::Document
    };
    SymbolRecord {
        id,
        pub_id: pub_id.to_string(),
        language: Language::Markdown,
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

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
