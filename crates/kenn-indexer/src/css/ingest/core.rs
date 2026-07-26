use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use kenn_config::CssConfig;
use kenn_model::id::css::{module_id, selector_id, SelectorKind};
use kenn_model::{EdgeProperties, EdgeRecord, ImportKind, Kind, Language, LinkGrade, ShortId};
use kenn_store::api::{DbError, Reader};
use kenn_store::DbReader;
use tokio::runtime::Handle;

use super::super::discover::{
    discover_stylesheets, discover_usage_sources, CssDiscoverError, DiscoveredStylesheet,
};
use super::super::extends::extract_extends;
use super::super::internal::{extract_imports, normalize_join, resolve_import};
use super::super::parse::{parse_css, CssIds};
use super::super::sass::{
    compile_and_extract_batch, discover_sass_compiler, is_sass_entry, SassExtract,
};
use super::super::usage::{
    class_name_candidates, extract_member_accesses, extract_module_bindings, extract_style_imports,
    is_stylesheet_import, resolve_usages, ClassRegistry,
};
use crate::sink::BatchSink;

/// State carried from the stylesheet producer to the post-code barrier: the
/// files to scan for class usage. (Usage edges mint no nodes, so no id allocator
/// is needed past the barrier — they only attach to existing code symbols.)
pub struct CssPending {
    pub usage_files: Vec<(PathBuf, String)>,
}

/// Counts from the post-barrier usage pass, for the run report.
#[derive(Debug, Default, Clone, Copy)]
pub struct CssUsageCounts {
    pub edges: u64,
    pub undefined: u64,
    /// Code→stylesheet `imports` edges (`import './x.css'`).
    pub import_edges: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CssIngestError {
    #[error(transparent)]
    Discover(#[from] CssDiscoverError),
    #[error("store append failed: {0}")]
    Store(#[from] DbError),
}

/// Record counts produced by a stylesheet ingest, for the run report.
#[derive(Debug, Default, Clone, Copy)]
pub struct CssCounts {
    pub files: u64,
    pub symbols: u64,
    pub defs: u64,
    pub edges: u64,
}

/// Discover stylesheets, parse/compile each, emit nodes/edges through `sink`,
/// and finish it. `.css` is parsed directly; `.scss`/`.sass` entry points are
/// compiled by dart-sass and extracted from the compiled output.
pub fn ingest_css_phase1(
    config: &CssConfig,
    workspace_root: &std::path::Path,
    mut sink: BatchSink,
) -> Result<(CssCounts, CssPending), CssIngestError> {
    let mut counts = CssCounts::default();
    let mut ids = CssIds::new();
    let discovered = discover_stylesheets(config, workspace_root)?;
    // relpath → its `module` node id, for resolving CSS-internal `@use`/`@import`.
    let mut module_map: HashMap<String, ShortId> = HashMap::new();
    // Every class node (pub_id, bare name, id), for resolving `@extend`/`composes`.
    let mut class_nodes: Vec<ClassNode> = Vec::new();
    let mut sass_files: Vec<&DiscoveredStylesheet> = Vec::new();

    for file in &discovered {
        if file.language == Language::Sass {
            // All sass files get a module node (for the imports graph); only
            // entries are compiled, partials reach the registry through them.
            sass_files.push(file);
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&file.abs_path) else {
            tracing::warn!(
                target: "kenn_indexer::css",
                path = %file.abs_path.display(),
                "unreadable stylesheet, skipped"
            );
            continue;
        };
        let Some(records) = parse_css(file, &content, &mut ids) else {
            tracing::warn!(
                target: "kenn_indexer::css",
                path = %file.relpath,
                "css parse failed, skipped"
            );
            continue;
        };
        if let Some(m) = records.symbols.first() {
            module_map.insert(file.relpath.clone(), m.id);
        }
        for s in &records.symbols {
            if s.kind == Kind::CssClass {
                class_nodes.push(ClassNode {
                    pub_id: s.pub_id.clone(),
                    name: s.name.clone(),
                    id: s.id,
                });
            }
        }
        counts.files += 1;
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

    let sass_modules = ingest_sass(
        config,
        workspace_root,
        &sass_files,
        &mut ids,
        &mut counts,
        &mut class_nodes,
        &mut sink,
    )?;
    module_map.extend(sass_modules);

    // CSS-internal graph: `@use`/`@import`/`@forward` → `imports` edges between
    // module nodes. Gated only on the stylesheet producer (no code barrier).
    emit_internal_imports(
        &discovered,
        &module_map,
        workspace_root,
        &mut counts,
        &mut sink,
    )?;

    // `@extend .class` / CSS-Modules `composes` → `extends_rule` edges between
    // class nodes (also CSS-internal, so before the barrier).
    emit_extends_edges(
        &discovered,
        &class_nodes,
        &module_map,
        workspace_root,
        &mut counts,
        &mut sink,
    )?;

    sink.finish()?;

    // Discover the files to scan for usage; the scan itself runs post-barrier.
    let usage_files = discover_usage_sources(config, workspace_root)?;
    if usage_files.is_empty() && config.usage_sources.is_empty() {
        tracing::info!(
            target: "kenn_indexer::css",
            "usage_sources is unset — class-usage mining is off; set [language.css] usage_sources to map where classes are used"
        );
    }
    Ok((counts, CssPending { usage_files }))
}

/// Post-code barrier (mirrors `resolve_markdown_code`): scan each `usage_sources`
/// file, intersect class-shaped tokens with the registry, and emit
/// `uses_css_class` edges from the enclosing code symbol to the class node.
/// `code` is the building snapshot + runtime handle, or `None` for a code-less
/// run (no source nodes → no edges). Owns and finishes `sink`.
pub fn resolve_css_usage(
    pending: CssPending,
    code: Option<(&DbReader, &Handle)>,
    mut sink: BatchSink,
) -> Result<CssUsageCounts, CssIngestError> {
    let mut counts = CssUsageCounts::default();
    let Some((reader, handle)) = code else {
        return Ok(counts); // no code graph → no source symbols to attach to
    };
    let registry = StoreClassRegistry { reader, handle };
    // Dedup code→stylesheet import edges across all files (one importer module may
    // import the same stylesheet on several lines).
    let mut seen_imports: HashSet<(ShortId, ShortId)> = HashSet::new();
    for (abs, relpath) in pending.usage_files {
        let Ok(content) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let Some(file_id) = handle
            .block_on(reader.fetch_file_short_id(&relpath))
            .ok()
            .flatten()
        else {
            continue; // file not in the code graph (e.g. not a code language)
        };
        // Fallback source when no enclosing symbol resolves at a usage line — the
        // file's module node, else the file node (spec §7.2). This matters in
        // practice: some code indexers (e.g. kenn-ts) emit declaration-line-only
        // def ranges, so a usage in a function *body* finds no enclosing symbol;
        // without the fallback the whole usage graph silently drops on real code.
        let fallback = module_of_file(reader, handle, file_id).unwrap_or(file_id);
        let scan = resolve_usages(&content, &registry, &|_| false);
        counts.undefined += scan.undefined.len() as u64;
        for hit in scan.hits {
            let line = offset_to_line(&content, hit.offset);
            let src = enclosing_or(reader, handle, file_id, line, fallback);
            sink.push_edge(EdgeRecord {
                src_id: src,
                target_id: hit.class_id,
                properties: EdgeProperties::UsesCssClass { grade: hit.grade },
            })?;
            counts.edges += 1;
        }
        emit_module_member_usages(
            reader,
            handle,
            file_id,
            &relpath,
            &content,
            fallback,
            &mut counts,
            &mut sink,
        )?;
        emit_code_style_imports(
            reader,
            handle,
            file_id,
            &relpath,
            &content,
            &mut seen_imports,
            &mut counts,
            &mut sink,
        )?;
    }
    sink.finish()?;
    Ok(counts)
}

/// Recover stylesheet imports a code indexer drops (`import './x.css'`): for each
/// stylesheet specifier in `content`, resolve the target `css`/`sass` module node
/// and emit an `imports` edge from the importing file's code module. Missing
/// targets and non-relative specifiers emit nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "barrier helper threads the store reader, handle, dedup set, counts, and sink"
)]
fn emit_code_style_imports(
    reader: &DbReader,
    handle: &Handle,
    file_id: ShortId,
    relpath: &str,
    content: &str,
    seen: &mut HashSet<(ShortId, ShortId)>,
    counts: &mut CssUsageCounts,
    sink: &mut BatchSink,
) -> Result<(), CssIngestError> {
    let style_imports = extract_style_imports(content);
    if style_imports.is_empty() {
        return Ok(());
    }
    let Some(src_module) = module_of_file(reader, handle, file_id) else {
        return Ok(()); // the importing file has no module node (shouldn't happen)
    };
    let dir = relpath.rsplit_once('/').map_or("", |(d, _)| d);
    for (spec, lang) in style_imports {
        if !spec.starts_with('.') {
            continue; // non-relative / aliased — path-mapping lives in the code indexer
        }
        let target_rel = normalize_join(dir, &spec);
        let pub_id = crate::pubid::floor(&module_id(lang, &target_rel).into_string());
        let Some(target) = handle
            .block_on(reader.fetch_symbol(lang.prefix(), &pub_id))
            .ok()
            .flatten()
        else {
            continue; // stylesheet not indexed → no edge (no dangling stub)
        };
        if target.id == src_module || !seen.insert((src_module, target.id)) {
            continue;
        }
        sink.push_edge(EdgeRecord {
            src_id: src_module,
            target_id: target.id,
            properties: EdgeProperties::Imports {
                kind: ImportKind::Explicit,
            },
        })?;
        counts.import_edges += 1;
    }
    Ok(())
}

/// The code `module` node that `contains` `file_id` (a code file has exactly one).
fn module_of_file(reader: &DbReader, handle: &Handle, file_id: ShortId) -> Option<ShortId> {
    handle
        .block_on(reader.list_inbound(
            file_id,
            "contains",
            1,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .ok()
        .and_then(|(rows, _)| rows.first().map(|r| r.id))
}

/// CSS-Modules binding resolution (JS/TS only): for `import s from
/// './x.module.css'` then `s.btnPrimary`, resolve the member to a class in THAT
/// stylesheet (camelCase↔kebab fold), graded `Exact`, attributed to the enclosing
/// code symbol. Non-JS files have no binding and rely on the plain token scan.
#[expect(
    clippy::too_many_arguments,
    reason = "barrier helper threads the store reader, handle, fallback source, counts, and sink"
)]
fn emit_module_member_usages(
    reader: &DbReader,
    handle: &Handle,
    file_id: ShortId,
    relpath: &str,
    content: &str,
    fallback: ShortId,
    counts: &mut CssUsageCounts,
    sink: &mut BatchSink,
) -> Result<(), CssIngestError> {
    if !is_js_ts(relpath) {
        return Ok(());
    }
    // Map each local binding to the stylesheet it imports (relpath + language).
    let dir = relpath.rsplit_once('/').map_or("", |(d, _)| d);
    let mut bound: HashMap<String, (String, Language)> = HashMap::new();
    for (local, spec) in extract_module_bindings(content) {
        if !spec.starts_with('.') {
            continue; // non-relative / aliased — out of scope
        }
        if let Some(lang) = is_stylesheet_import(&spec) {
            bound.insert(local, (normalize_join(dir, &spec), lang));
        }
    }
    if bound.is_empty() {
        return Ok(());
    }
    let locals: HashSet<String> = bound.keys().cloned().collect();
    for (local, member, offset) in extract_member_accesses(content, &locals) {
        let Some((target_rel, lang)) = bound.get(&local) else {
            continue;
        };
        let Some(class_id) = resolve_member_class(reader, handle, *lang, target_rel, &member)
        else {
            continue; // member matches no class in the bound file → no edge
        };
        let line = offset_to_line(content, offset);
        let src = enclosing_or(reader, handle, file_id, line, fallback);
        sink.push_edge(EdgeRecord {
            src_id: src,
            target_id: class_id,
            properties: EdgeProperties::UsesCssClass {
                grade: LinkGrade::Exact,
            },
        })?;
        counts.edges += 1;
    }
    Ok(())
}

/// The smallest symbol enclosing `line` in `file_id`, or `fallback` when none
/// resolves (spec §7.2: usage attaches to the enclosing symbol when resolvable,
/// otherwise the containing module / file node).
fn enclosing_or(
    reader: &DbReader,
    handle: &Handle,
    file_id: ShortId,
    line: u32,
    fallback: ShortId,
) -> ShortId {
    handle
        .block_on(reader.find_at_location(file_id, line))
        .ok()
        .and_then(|rows| rows.first().map(|s| s.id))
        .unwrap_or(fallback)
}

/// Resolve a CSS-module member to a `css_class` node in `target_rel`, trying the
/// camelCase↔kebab folds; returns the first candidate that exists.
fn resolve_member_class(
    reader: &DbReader,
    handle: &Handle,
    lang: Language,
    target_rel: &str,
    member: &str,
) -> Option<ShortId> {
    for cand in class_name_candidates(member) {
        let pub_id = crate::pubid::floor(
            &selector_id(lang, target_rel, SelectorKind::Class, &cand).into_string(),
        );
        if let Some(sym) = handle
            .block_on(reader.fetch_symbol(lang.prefix(), &pub_id))
            .ok()
            .flatten()
        {
            return Some(sym.id);
        }
    }
    None
}

/// Whether `relpath` is a JS/TS source (the only place CSS-module bindings apply).
fn is_js_ts(relpath: &str) -> bool {
    let ext = relpath.rsplit_once('.').map_or("", |(_, e)| e);
    matches!(
        ext,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
    )
}

/// [`ClassRegistry`] over the building store: a class name → the `css_class`
/// node ids that define it (filtered from `symbols_by_short_name` by the typed
/// `#class:` pub-id fragment). Each query blocks the calling thread on the async
/// reader, mirroring markdown's `StoreCodeLookup`.
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

/// 1-based line number of byte `offset` within `content`.
fn offset_to_line(content: &str, offset: usize) -> u32 {
    let prefix = content.get(..offset).unwrap_or(content);
    1 + u32::try_from(prefix.bytes().filter(|&b| b == b'\n').count()).unwrap_or(u32::MAX - 1)
}

/// Compile the Sass entry points with dart-sass and emit the extracted nodes.
/// Returns the `relpath → module id` map for every origin file (entries +
/// reached partials), for CSS-internal import resolution. No-op (with a log)
/// when there are no entries or no compiler is found.
fn ingest_sass(
    config: &CssConfig,
    workspace_root: &std::path::Path,
    sass_files: &[&DiscoveredStylesheet],
    ids: &mut CssIds,
    counts: &mut CssCounts,
    class_nodes: &mut Vec<ClassNode>,
    sink: &mut BatchSink,
) -> Result<HashMap<String, ShortId>, CssIngestError> {
    if sass_files.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(compiler) = discover_sass_compiler(&config.sass, workspace_root) else {
        tracing::warn!(
            target: "kenn_indexer::css",
            count = sass_files.len(),
            "sass files found but no dart-sass compiler discovered (node_modules/.bin/sass, PATH, …); skipped"
        );
        return Ok(HashMap::new());
    };

    let mut acc = SassExtract::default();
    // Compile all entry points (non-`_`-prefixed) in ONE dart-sass invocation;
    // partials come via the source map. Batching avoids a subprocess per entry.
    let entries: Vec<&DiscoveredStylesheet> = sass_files
        .iter()
        .copied()
        .filter(|f| is_sass_entry(&f.relpath))
        .collect();
    compile_and_extract_batch(
        &compiler,
        &entries,
        &config.sass.load_paths,
        workspace_root,
        ids,
        &mut acc,
    );
    // Ensure every sass file has a module node (incl. import-only barrels and
    // functions-only partials), so the imports graph has both endpoints.
    for file in sass_files {
        acc.ensure_module(&file.relpath, workspace_root, ids);
    }

    for s in &acc.symbols {
        if s.kind == Kind::CssClass {
            class_nodes.push(ClassNode {
                pub_id: s.pub_id.clone(),
                name: s.name.clone(),
                id: s.id,
            });
        }
    }
    counts.files += acc.file_count();
    counts.symbols += acc.symbols.len() as u64;
    counts.defs += acc.defs.len() as u64;
    counts.edges += acc.edges.len() as u64;
    let module_map = acc.module_map();
    sink.push_document_records(acc.files, acc.symbols, acc.docs, acc.defs, acc.edges)?;
    Ok(module_map)
}

/// One class node, indexed for `@extend`/`composes` resolution.
struct ClassNode {
    pub_id: String,
    name: String,
    id: ShortId,
}

/// Scan each stylesheet for `@extend .class` / CSS-Modules `composes` and emit
/// an `extends_rule` edge (extending class → extended class). The enclosing
/// class must be defined in the same file; the target resolves by exact `pub_id`
/// (`composes … from './x'`, same-file) or by bare name across the corpus
/// (Sass `@extend`, keep-all when ambiguous). A target that resolves to nothing
/// emits no edge.
fn emit_extends_edges(
    discovered: &[DiscoveredStylesheet],
    class_nodes: &[ClassNode],
    module_map: &HashMap<String, ShortId>,
    workspace_root: &std::path::Path,
    counts: &mut CssCounts,
    sink: &mut BatchSink,
) -> Result<(), CssIngestError> {
    let by_pubid: HashMap<&str, ShortId> = class_nodes
        .iter()
        .map(|c| (c.pub_id.as_str(), c.id))
        .collect();
    let mut by_name: HashMap<&str, Vec<ShortId>> = HashMap::new();
    for c in class_nodes {
        by_name.entry(c.name.as_str()).or_default().push(c.id);
    }

    for file in discovered {
        let Ok(source) = std::fs::read_to_string(workspace_root.join(&file.relpath)) else {
            continue;
        };
        for r in extract_extends(&source) {
            let src_pub = crate::pubid::floor(
                &selector_id(
                    file.language,
                    &file.relpath,
                    SelectorKind::Class,
                    &r.enclosing,
                )
                .into_string(),
            );
            let Some(&src_id) = by_pubid.get(src_pub.as_str()) else {
                continue; // enclosing class not a known node in this file
            };
            let targets = resolve_extend_target(&r, file, &by_pubid, &by_name, module_map);
            let grade = if targets.len() > 1 {
                LinkGrade::Ambiguous
            } else {
                LinkGrade::Exact
            };
            for target_id in targets {
                if target_id == src_id {
                    continue;
                }
                sink.push_edge(EdgeRecord {
                    src_id,
                    target_id,
                    properties: EdgeProperties::ExtendsRule { grade },
                })?;
                counts.edges += 1;
            }
        }
    }
    Ok(())
}

/// Resolve an `@extend`/`composes` target to the class node ids it names.
/// `composes … from './x'` resolves the file then the exact class; everything
/// else prefers a same-file def, falling back to all same-named defs.
fn resolve_extend_target(
    r: &super::super::extends::ExtendRef,
    file: &DiscoveredStylesheet,
    by_pubid: &HashMap<&str, ShortId>,
    by_name: &HashMap<&str, Vec<ShortId>>,
    module_map: &HashMap<String, ShortId>,
) -> Vec<ShortId> {
    if let Some(spec) = &r.from {
        let Some((target_rel, _)) = resolve_import(&file.relpath, spec, module_map) else {
            return Vec::new();
        };
        let pub_id = crate::pubid::floor(
            &selector_id(file.language, &target_rel, SelectorKind::Class, &r.target).into_string(),
        );
        return by_pubid.get(pub_id.as_str()).copied().into_iter().collect();
    }
    let same = crate::pubid::floor(
        &selector_id(file.language, &file.relpath, SelectorKind::Class, &r.target).into_string(),
    );
    if let Some(&id) = by_pubid.get(same.as_str()) {
        vec![id]
    } else {
        by_name.get(r.target.as_str()).cloned().unwrap_or_default()
    }
}

/// Pass B: scan each stylesheet source for `@use`/`@import`/`@forward` and emit
/// an `imports` edge (importer module → imported module) when the specifier
/// resolves to a known module. Missing targets emit no edge.
fn emit_internal_imports(
    discovered: &[DiscoveredStylesheet],
    module_map: &HashMap<String, ShortId>,
    workspace_root: &std::path::Path,
    counts: &mut CssCounts,
    sink: &mut BatchSink,
) -> Result<(), CssIngestError> {
    for file in discovered {
        let Some(&src_id) = module_map.get(&file.relpath) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(workspace_root.join(&file.relpath)) else {
            continue;
        };
        for spec in extract_imports(&source) {
            if let Some((_, target_id)) = resolve_import(&file.relpath, &spec, module_map) {
                if target_id != src_id {
                    sink.push_edge(EdgeRecord {
                        src_id,
                        target_id,
                        properties: EdgeProperties::Imports {
                            kind: ImportKind::Explicit,
                        },
                    })?;
                    counts.edges += 1;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
