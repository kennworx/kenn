//! The per-`Document` transform: walk one SCIP `Document` into
//! `kenn_model` records (file, symbols, docs, defs, edges) plus the
//! Rust file-doc extraction and definition-occurrence prepass it relies on.

use std::collections::{HashMap, HashSet};

use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FileDocsRecord, FileRecord, Language, PackageRecord,
    ShortId, SymbolDocsRecord, SymbolRecord,
};
use scip::types::Document;

use crate::canonicalize::Workspace;

use crate::transform::{
    derive_display_name, derive_kind, derive_short_name, intern_symbol_with_stub,
    is_test_descriptor, language_from_path, language_from_scip, parent_scip_symbol,
    transformer_for, IdRegistry, TransformError,
};

/// Transform a single SCIP `Document` into `kenn_model` records.
///
/// Out-params (returned via `Vec`) instead of an `impl Iterator` so the
/// caller can decide ordering/streaming. The volume per doc is bounded —
/// even for 1k-symbol files, all four buffers stay under a megabyte.
#[derive(Default)]
pub struct TransformedDocument {
    /// `None` when this SCIP `Document` repeats a file path that an earlier
    /// document already produced — the caller should skip emitting a
    /// `FileRecord` to avoid violating the `files_path` unique index.
    pub file: Option<FileRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub docs: Vec<SymbolDocsRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    /// File-level comment docs (Rust only). SCIP carries no file-level
    /// comments, so these are read from source on disk for new files and
    /// run through the same license filter as the C# path.
    pub file_docs: Vec<FileDocsRecord>,
    /// Packages first seen in this document. The SCIP path used to emit none —
    /// every symbol carried `pkg_id: 0` — so `packages` was empty for rust, go
    /// and python, `--package` filters matched nothing, and the atlas had to
    /// infer packages from manifest directories.
    pub packages: Vec<PackageRecord>,
}

#[expect(
    clippy::too_many_lines,
    reason = "linear SCIP-walk; splitting hurts readability"
)]
pub fn transform_document(
    doc: &Document,
    workspace: &Workspace,
    project_root_uri: &str,
    registry: &mut IdRegistry,
) -> Result<TransformedDocument, TransformError> {
    let language = language_from_scip(&doc.language)
        .or_else(|| {
            // scip-typescript 0.4.0 emits empty Document.language; fall back to extension.
            doc.language
                .is_empty()
                .then(|| language_from_path(&doc.relative_path))
                .flatten()
        })
        .ok_or_else(|| TransformError::UnknownLanguage(doc.language.clone()))?;
    // Per-language `[language.X].excludes` — workspace-relative glob
    // filter scoped to language X's transform. Drop the entire Document
    // (no SymbolRecord / DefRecord / occurrence edge) when its
    // relative_path matches X's exclude set. Cross-document edges
    // referencing dropped symbols still emit via the separate
    // external_symbols frame in ingest_scip_into_sink.
    if workspace.is_excluded(language, &doc.relative_path) {
        tracing::debug!(
            target: "kenn_indexer::transform",
            language = ?language,
            path = %doc.relative_path,
            "dropped document via [language.{lang}].excludes",
            lang = language.db_name(),
        );
        return Ok(TransformedDocument::default());
    }
    let canonical = workspace.canonicalize(project_root_uri, &doc.relative_path)?;
    let rel = canonical.into_string();
    let is_test_file = workspace.is_test_path(&rel);

    let (file_id, is_new_file) = registry.intern_file_with_seen(&rel);
    // SCIP carries no file-level comments. For a new Rust file, read the
    // source from disk and extract the leading comment blocks (license
    // header + `//!` module docs), then run them through the same license
    // filter the C# path uses.
    let mut file_docs = Vec::new();
    if is_new_file && language == Language::Rust {
        if let Ok(src) = std::fs::read_to_string(workspace.root().join(&rel)) {
            let blocks = extract_rust_file_docs(&src);
            if let Some(rec) = crate::transform_jsonl::file_doc_record(file_id, &rel, &blocks) {
                file_docs.push(rec);
            }
        }
    }
    let file = is_new_file.then_some(FileRecord {
        id: file_id,
        path: rel,
        language,
        test: is_test_file,
        external: false,
        // The producer doesn't have file bytes here; storage layer hashes
        // on persist. Default 0 — `source-data-model` D4 says non-zero is
        // assigned at ingest, which happens in the indexed-store layer.
        content_hash: 0,
    });

    let transformer = transformer_for(language)
        .ok_or_else(|| TransformError::UnknownLanguage(format!("{language:?}")))?;
    let mut symbols = Vec::with_capacity(doc.symbols.len());
    let mut docs = Vec::new();
    let mut defs = Vec::new();
    let mut edges = Vec::new();
    let mut packages = Vec::new();

    // Prepass: collect this document's Definition occurrences keyed by SCIP
    // symbol. The per-symbol loop below pushes one `DefRecord` per occurrence,
    // converting SCIP's 0-based lines to the store's 1-based convention
    // (`source-data-model` D1). Columns pass through unchanged (0-based).
    let def_occurrences = collect_definition_occurrences(doc);

    for info in &doc.symbols {
        let public_id = match transformer.scip_to_public(&info.symbol) {
            Ok(pid) => pid,
            Err(_) if info.symbol.starts_with("local ") => {
                // SCIP local symbols are intra-document; we don't expose them.
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let (short_id, is_new) =
            registry.intern_with_pub_id(language, &info.symbol, public_id.as_str());

        // Push one DefRecord per Definition occurrence found for this symbol
        // in this document, regardless of `is_new` — cfg-gated partials across
        // documents each contribute their own row (scip-indexer D2.4). When
        // no Definition occurrence exists in this document (pathological
        // SCIP), fall back to a zero-range placeholder so the symbol still
        // has a `defs` row anchored to `file_id`.
        push_def_records(&mut defs, short_id, file_id, &def_occurrences, &info.symbol);

        if !is_new {
            // A different SCIP string already produced this (language, pub_id)
            // — typical of multi-csproj solutions where the same conceptual
            // symbol is re-emitted from each project. Edge derivation still
            // resolves to the same short_id; we just skip the duplicate
            // SymbolRecord and SymbolDocsRecord. The DefRecord above already
            // captured this document's contribution.
            continue;
        }
        let name = derive_display_name(&info.symbol)
            .map_or_else(|| public_id.as_str().into(), |d| derive_short_name(&d));

        let kind = derive_kind(info);
        let is_test_sym = is_test_file || is_test_descriptor(language, kind, public_id.as_str());
        // Resolve enclosing parent from the SCIP descriptor — the JSONL
        // path sets this from explicit frames, but SCIP doesn't carry
        // it; without it the aggregation roll-up (methods → class,
        // fields → class, free fns → module) is dead. Buffer a stub
        // for the parent if it's not in this document; the parent's
        // full SymbolRecord (when seen later, in this or another doc)
        // clears the stub via `mark_full_emitted`. See
        // `intern_symbol_with_stub`.
        let enclosing_symbol = parent_scip_symbol(&info.symbol).map_or(0, |parent| {
            intern_symbol_with_stub(registry, language, &parent)
        });
        // We have a real SymbolRecord for this id — clear any prior stub
        // we may have buffered for it from a previous interning.
        registry.mark_full_emitted(short_id);
        let pkg_id = intern_package(&info.symbol, language, registry, &mut packages);
        symbols.push(SymbolRecord {
            id: short_id,
            pub_id: crate::pubid::render(language, public_id.as_str()),
            language,
            pkg_id,
            kind,
            name,
            enclosing_sym_id: enclosing_symbol,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: is_test_sym,
        });
        let signature_doc = info
            .signature_documentation
            .as_ref()
            .map(format_signature_documentation)
            .unwrap_or_default();
        if !signature_doc.is_empty() || !info.documentation.is_empty() {
            docs.push(SymbolDocsRecord {
                sym_id: short_id,
                sig: signature_doc,
                doc: info.documentation.join("\n"),
            });
        }

        // Task 4.3 — explicit relationships (`extends`, `implements`, ...)
        // SCIP encodes these on `SymbolInformation.relationships`.
        for rel in &info.relationships {
            let target = intern_symbol_with_stub(registry, language, &rel.symbol);
            if rel.is_implementation {
                edges.push(EdgeRecord {
                    src_id: short_id,
                    target_id: target,
                    properties: EdgeProperties::Implements,
                });
            }
            if rel.is_reference {
                edges.push(EdgeRecord {
                    src_id: short_id,
                    target_id: target,
                    properties: EdgeProperties::TypeUse,
                });
            }
            if rel.is_definition {
                // Per SCIP: this symbol is an alternate definition of `target`.
                // Materialize as a `corresponds_to` config-source pair.
                edges.push(EdgeRecord {
                    src_id: short_id,
                    target_id: target,
                    properties: EdgeProperties::CorrespondsTo {
                        source: kenn_model::IsomorphismSource::Config,
                        generator: String::new(),
                        canonical: target,
                    },
                });
            }
        }
    }

    if language == Language::Rust {
        push_rust_trait_impl_edges(doc, registry, &mut edges);
    }

    Ok(TransformedDocument {
        file,
        symbols,
        docs,
        defs,
        edges,
        file_docs,
        packages,
    })
}

/// Intern the package a SCIP symbol belongs to, buffering a `PackageRecord` the
/// first time it is seen, and return its id (`0` when the moniker names none).
///
/// The identity comes from the moniker itself — the crate for Rust, the
/// distribution for Python, the import path for Go — so nothing new has to be
/// discovered. `external` is false because a document being transformed is
/// first-party by construction; cross-crate references reach
/// `intern_symbol_with_stub` instead, which marks its stubs external.
fn intern_package(
    scip_symbol: &str,
    language: Language,
    registry: &mut IdRegistry,
    out: &mut Vec<PackageRecord>,
) -> ShortId {
    let Some((name, version)) = kenn_model::id::package::package_of(language, scip_symbol) else {
        return 0;
    };
    let (id, is_new) = registry.intern_package(&name, version);
    if is_new {
        out.push(PackageRecord {
            id,
            name,
            version: version.to_string(),
            manager: String::new(),
            external: false,
        });
    }
    id
}

/// Emit `Implements` edges for the trait impls in a rust-analyzer document.
///
/// rust-analyzer does NOT populate SCIP `SymbolInformation.relationships` — the
/// channel the loop above reads, and the one scip-go and scip-python do fill —
/// so without this pass Rust is the only indexed language that yields zero
/// implements edges, and `kenn list implementers` on a trait always answers
/// empty. The relationship does survive, structurally, in the moniker:
/// `impl#[Type][Trait]member` ([`trait_impl_of`]).
///
/// Both names in that moniker are BARE — rust-analyzer never qualifies the
/// trait with its defining crate, so `[Default]` alone cannot say whether it
/// means `std`'s or a local one. Resolution is therefore **document-scoped**:
/// `impl Default for Foo` necessarily references the real `Default` in this same
/// document, so the document's own type references are a small, precise
/// candidate set. A name matching no reference, or more than one, is SKIPPED —
/// two same-named traits referenced in one file is the ambiguity this cannot
/// resolve, and guessing would attach the impl to the wrong trait.
fn push_rust_trait_impl_edges(
    doc: &Document,
    registry: &mut IdRegistry,
    edges: &mut Vec<EdgeRecord>,
) {
    use kenn_model::id::{base_type_name, terminal_type_name, trait_impl_of};
    use protobuf::Enum;
    use scip::types::symbol_information::Kind as ScipKind;

    use crate::edge::is_pseudo_symbol;

    // Nothing to resolve unless this document actually declares a trait impl —
    // and occurrences outnumber symbols by an order of magnitude, so checking the
    // smaller collection first skips the index build entirely for the many files
    // that contain no `impl … for …`.
    if !doc
        .symbols
        .iter()
        .any(|i| trait_impl_of(&i.symbol).is_some())
    {
        return;
    }

    // Symbols this document DEFINES that are demonstrably not traits. A trait
    // name must never resolve to one of these: when the real trait produces no
    // occurrence here (a macro- or derive-expanded impl, a trait reached only
    // through an aliased re-export), a same-named struct would otherwise be the
    // unique candidate and we would emit an edge claiming it is implemented.
    // Only same-document definitions carry a kind; cross-crate references have
    // none, so this narrows the candidate set without being able to close it.
    let not_a_trait: HashSet<&str> = doc
        .symbols
        .iter()
        .filter(|i| {
            matches!(
                ScipKind::from_i32(i.kind.value()),
                Some(ScipKind::Struct | ScipKind::Class | ScipKind::Enum | ScipKind::Union)
            )
        })
        .map(|i| i.symbol.as_str())
        .collect();

    // Candidate index: every type this document mentions, by base name. Built
    // from occurrences (not `doc.symbols`) so a trait defined in another crate —
    // the common case, and the whole reason a workspace-only trait table would
    // drop two thirds of Rust's impls — is still a candidate here.
    let mut by_name: HashMap<&str, Option<&str>> = HashMap::new();
    for occ in &doc.occurrences {
        if is_pseudo_symbol(&occ.symbol) {
            continue;
        }
        if let Some(name) = terminal_type_name(&occ.symbol) {
            by_name
                .entry(name)
                // `Some(sym)` = one candidate so far; `None` = ambiguous. A
                // repeat of the SAME symbol is not ambiguity — the same type is
                // referenced many times in a file.
                .and_modify(|slot| {
                    if *slot != Some(occ.symbol.as_str()) {
                        *slot = None;
                    }
                })
                .or_insert(Some(occ.symbol.as_str()));
        }
    }
    let resolve =
        |name: &str| -> Option<&str> { by_name.get(base_type_name(name)).copied().flatten() };

    // One impl block declares many members; every one carries the same
    // `impl#[T][Tr]` prefix, so dedup before interning.
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for info in &doc.symbols {
        let Some(imp) = trait_impl_of(&info.symbol) else {
            continue;
        };
        let (Some(ty), Some(tr)) = (resolve(imp.type_name), resolve(imp.trait_name)) else {
            continue;
        };
        if ty == tr || not_a_trait.contains(tr) || !seen.insert((ty, tr)) {
            continue;
        }
        let src_id = intern_symbol_with_stub(registry, Language::Rust, ty);
        let target_id = intern_symbol_with_stub(registry, Language::Rust, tr);
        edges.push(EdgeRecord {
            src_id,
            target_id,
            properties: EdgeProperties::Implements,
        });
    }
}

/// Extract leading file-level comment blocks from Rust source: the header
/// license block, `//!` inner module docs, and a top `/* */` block.
/// Contiguous `//` lines coalesce into one block; a blank line breaks the
/// block; inner `#![…]` attributes are skipped (so module docs below them
/// are still captured); the first line of real code ends the scan. Mirrors
/// the C# `FileDoc.Extract` block semantics so both languages feed the same
/// license filter.
pub(crate) fn extract_rust_file_docs(source: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with("//") {
            cur.push(line.trim_end());
        } else if t.starts_with("/*") {
            flush_block(&mut cur, &mut blocks);
            let mut blk = vec![line.trim_end()];
            if !t.contains("*/") {
                for l in lines.by_ref() {
                    blk.push(l.trim_end());
                    if l.contains("*/") {
                        break;
                    }
                }
            }
            blocks.push(blk.join("\n"));
        } else if t.is_empty() || t.starts_with("#!") {
            // Blank line breaks the current block; an inner attribute is
            // skipped but does not end the leading region.
            flush_block(&mut cur, &mut blocks);
        } else {
            break;
        }
    }
    flush_block(&mut cur, &mut blocks);
    blocks
}

fn flush_block(cur: &mut Vec<&str>, blocks: &mut Vec<String>) {
    if !cur.is_empty() {
        blocks.push(cur.join("\n"));
        cur.clear();
    }
}

/// SCIP `signature_documentation` is itself a `Document` with one occurrence
/// whose `text`-bearing field holds the rendered signature. Concatenate any
/// inline text the indexer emitted.
fn format_signature_documentation(sig_doc: &Document) -> String {
    sig_doc.text.clone()
}

/// One Definition occurrence: the NAME range (four-tuple) plus the optional
/// enclosing-item BODY line span (`(start_line, end_line)`, 0-based) that
/// rust-analyzer ≥ Dec-2025 / scip-go / scip-python stamp on definitions via
/// `Occurrence.enclosing_range`. `None` when the producer emits none.
struct DefOccurrence {
    name: (i32, i32, i32, i32),
    body_lines: Option<(i32, i32)>,
}

/// Collect this document's Definition occurrences keyed by SCIP symbol —
/// the input to `push_def_records`. Pseudo (`local …`) symbols and ranges
/// of unexpected shape are skipped.
fn collect_definition_occurrences(doc: &Document) -> HashMap<&str, Vec<DefOccurrence>> {
    let mut by_sym: HashMap<&str, Vec<DefOccurrence>> = HashMap::new();
    for occ in &doc.occurrences {
        if !crate::edge::is_definition(occ) || crate::edge::is_pseudo_symbol(&occ.symbol) {
            continue;
        }
        let name = match *occ.range.as_slice() {
            [sl, sc, ec] => (sl, sc, sl, ec),
            [sl, sc, el, ec] => (sl, sc, el, ec),
            _ => continue,
        };
        // `enclosing_range` shares `range`'s shape (3-int single-line or
        // 4-int multi-line). Empty for older rust-analyzer / synthetic defs.
        let body_lines = match *occ.enclosing_range.as_slice() {
            [sl, _, _] => Some((sl, sl)),
            [sl, _, el, _] => Some((sl, el)),
            _ => None,
        };
        by_sym
            .entry(occ.symbol.as_str())
            .or_default()
            .push(DefOccurrence { name, body_lines });
    }
    by_sym
}

/// Append one `DefRecord` per Definition occurrence of `scip_symbol` in the
/// prepass map. Falls back to a single zero-range placeholder when the map
/// has no entry (so every workspace-defined symbol still has a `defs` row
/// anchored to `file_id`). 0-based SCIP lines convert to 1-based stored
/// lines per `source-data-model` D1; the zero-tuple stays zero.
fn push_def_records(
    defs: &mut Vec<DefRecord>,
    sym_id: ShortId,
    file_id: ShortId,
    by_sym: &HashMap<&str, Vec<DefOccurrence>>,
    scip_symbol: &str,
) {
    let to_u32 = |v: i32| -> u32 { u32::try_from(v.max(0)).unwrap_or(0) };
    let fallback = [DefOccurrence {
        name: (0, 0, 0, 0),
        body_lines: None,
    }];
    let real_occurrences = by_sym.get(scip_symbol).map(Vec::as_slice);
    // A symbol with no SCIP Definition occurrence is synthetic/external —
    // emit the `[0,0,0,0]` placeholder per the spec, paired with the
    // caller's `is_external = true` marking. A symbol WITH a Definition
    // occurrence is always 1-based 1..N, even when the occurrence's
    // range is literally `[0,0,0,0]` (legal scip-python output for
    // module-level symbols defined on line 1 column 0).
    let (occurrences, is_synthetic) =
        real_occurrences.map_or((&fallback[..], true), |s| (s, false));
    for occ in occurrences {
        let (sl, sc, el, ec) = occ.name;
        let (start_line, end_line) = if is_synthetic {
            (0, 0)
        } else {
            (to_u32(sl) + 1, to_u32(el) + 1)
        };
        // Enclosing-item body span (1-based). Absent → 0 (get_source falls
        // back to the name span). Never emitted for synthetic placeholders.
        let (body_start_line, body_end_line) = match occ.body_lines {
            Some((bsl, bel)) if !is_synthetic => (to_u32(bsl) + 1, to_u32(bel) + 1),
            _ => (0, 0),
        };
        defs.push(DefRecord {
            sym_id,
            file_id,
            start_line,
            start_col: to_u32(sc),
            end_line,
            end_col: to_u32(ec),
            body_start_line,
            body_end_line,
        });
    }
}
