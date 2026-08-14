//! The `.xml` producer: a barrier-free phase-1 sibling unit.
//!
//! Discovery, parsing, and writing complete in one pass with no pending state
//! for the post-code barrier. A later bridge reads the element nodes back from
//! the building store rather than being handed deferred state, which is what
//! keeps this producer barrier-free.
//!
//! **Every element is a node; no attribute is.** Attributes and text ride on
//! the element, which is what bounds the graph to the element count — measured
//! on a real repository, 30410 elements across 485 files, where attributes
//! outnumber elements several times over.
//!
//! **Two surfaces, like code.** The rendered start tag is the signature; the
//! element's own text is the content. Storing both flattened into one string
//! made the content unusable to any consumer that needs to parse it — a `<sql>`
//! body reaches `sqlparser` as `sql ALTER TABLE users` and is rejected at token
//! 1 — and made attributes unrecoverable, since a flattened value containing a
//! space is indistinguishable from two words.
//!
//! Neither surface is embedded. XML content is configuration values rather than
//! prose, so vectors would cost a pass on every run and dilute the conceptual
//! recall they exist for; the store excludes the language from embedding
//! selection and derives a verbatim lexical projection from both surfaces
//! instead. Only 1.8% of real elements carry any text at all.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_model::id::xml::{document_id, element_id};
use kenn_model::{
    compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, ShortId,
    SymbolDocsRecord, SymbolRecord,
};
use thiserror::Error;

use super::parse::{parse, signature};
use crate::sink::BatchSink;

#[derive(Debug, Error)]
pub enum XmlIngestError {
    #[error("bad {kind} glob `{pattern}`: {source}")]
    BadGlob {
        kind: &'static str,
        pattern: String,
        source: globset::Error,
    },
    #[error(transparent)]
    Db(#[from] kenn_store::DbError),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct XmlCounts {
    pub files: u64,
    pub elements: u64,
    pub edges: u64,
    /// Files that could not be read or parsed. Each names its position.
    pub failed: u64,
}

struct XmlIds {
    next_file: u32,
    next_symbol: u32,
}

impl XmlIds {
    fn new() -> Self {
        Self {
            next_file: 1,
            next_symbol: 1,
        }
    }
    fn file(&mut self) -> ShortId {
        let id = compose_short_id(Language::Xml, self.next_file);
        self.next_file += 1;
        id
    }
    fn symbol(&mut self) -> ShortId {
        let id = compose_short_id(Language::Xml, self.next_symbol);
        self.next_symbol += 1;
        id
    }
}

fn build_set(patterns: &[String], kind: &'static str) -> Result<GlobSet, XmlIngestError> {
    let mut b = GlobSetBuilder::new();
    for pat in patterns {
        let g = Glob::new(pat).map_err(|source| XmlIngestError::BadGlob {
            kind,
            pattern: pat.clone(),
            source,
        })?;
        b.add(g);
    }
    b.build().map_err(|source| XmlIngestError::BadGlob {
        kind,
        pattern: String::new(),
        source,
    })
}

/// Collect claimed files under `root`, honouring the exclude set.
fn discover(root: &Path, exts: &[String], excludes: &GlobSet, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if excludes.is_match(path.to_string_lossy().as_ref()) {
            continue;
        }
        if path.is_dir() {
            discover(&path, exts, excludes, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.iter().any(|c| c == &e.to_ascii_lowercase()))
        {
            out.push(path);
        }
    }
}

/// Index every claimed XML file under the workspace, emitting through `sink`.
///
/// # Errors
/// Returns an error only for a bad configured glob or a store write failure —
/// an unreadable or malformed file degrades the counts and the run continues.
pub fn ingest_xml(
    config: &kenn_config::XmlConfig,
    workspace_root: &Path,
    mut sink: BatchSink,
) -> Result<XmlCounts, XmlIngestError> {
    let mut counts = XmlCounts::default();
    let excludes = build_set(&config.effective_excludes(), "exclude")?;
    let exts = config.claimed_extensions();
    let mut paths = Vec::new();
    discover(workspace_root, &exts, &excludes, &mut paths);
    if paths.is_empty() {
        sink.finish()?;
        return Ok(counts);
    }

    let mut ids = XmlIds::new();
    for abs in &paths {
        let Ok(content) = std::fs::read_to_string(abs) else {
            counts.failed += 1;
            tracing::warn!(target: "kenn_indexer::xml", path = %abs.display(), "unreadable xml file, skipped");
            continue;
        };
        let elements = match parse(&content) {
            Ok(e) => e,
            Err(err) => {
                // One malformed file must not cost the others; the report names
                // the file and the position the parser gave.
                counts.failed += 1;
                tracing::warn!(
                    target: "kenn_indexer::xml",
                    path = %abs.display(),
                    error = %err,
                    "malformed xml, skipped"
                );
                continue;
            }
        };
        if elements.is_empty() {
            continue;
        }
        let relpath = abs
            .strip_prefix(workspace_root)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/");

        let file_id = ids.file();
        let doc_sym = ids.symbol();
        let total_lines = u32::try_from(content.lines().count())
            .unwrap_or(u32::MAX)
            .max(1);

        sink.push_file(FileRecord {
            id: file_id,
            path: relpath.clone(),
            language: Language::Xml,
            test: false,
            external: false,
            content_hash: xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
        })?;
        // `Document` is an aggregate leaf, so every element below rolls up into
        // its file rather than becoming its own aggregate — the collapse that
        // keeps a numerically dominant document language from distorting the
        // atlas (measured 30410 elements to 483 aggregates).
        sink.push_symbol(SymbolRecord {
            id: doc_sym,
            // Floored here, not in the id module: shell-safety is per-ingester
            // through the one shared `pubid::floor`, so XML ids render like
            // every other language's for the same input. Both id sources are
            // arbitrary text — the file path here, an attribute value below.
            pub_id: crate::pubid::floor(document_id(&relpath).as_str()),
            language: Language::Xml,
            pkg_id: 0,
            kind: Kind::Document,
            name: stem(&relpath),
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
        })?;
        sink.push_def(def(doc_sym, file_id, 1, total_lines))?;
        counts.files += 1;

        emit_elements(
            &mut sink,
            &elements,
            &relpath,
            &content,
            doc_sym,
            file_id,
            &mut ids,
            &mut counts,
        )?;
    }

    sink.finish()?;
    Ok(counts)
}

/// Emit one node per element, each parented to its enclosing element (or the
/// document, for a root). The chain is what makes elements roll up into their
/// file rather than each becoming its own aggregate.
#[expect(
    clippy::too_many_arguments,
    reason = "one call site; grouping these into a struct would only move the same fields"
)]
fn emit_elements(
    sink: &mut BatchSink,
    elements: &[super::parse::Element],
    relpath: &str,
    content: &str,
    doc_sym: ShortId,
    file_id: ShortId,
    ids: &mut XmlIds,
    counts: &mut XmlCounts,
) -> Result<(), XmlIngestError> {
    // Element index → short id, so a child can point at its parent.
    let mut sym_of: Vec<ShortId> = Vec::with_capacity(elements.len());
    {
        for el in elements {
            let sym = ids.symbol();
            sym_of.push(sym);
            let enclosing = el.parent.and_then(|i| sym_of.get(i).copied());
            let (start_line, end_line) = line_span(content, el.span.start, el.span.end);

            sink.push_symbol(SymbolRecord {
                id: sym,
                pub_id: crate::pubid::floor(element_id(relpath, &el.chain).as_str()),
                language: Language::Xml,
                pkg_id: 0,
                kind: Kind::XmlElement,
                name: el.tag.clone(),
                // Root elements hang off the document; the rest off their parent.
                enclosing_sym_id: enclosing.unwrap_or(doc_sym),
                partial: false,
                nargs: 0,
                targs: 0,
                external: false,
                test: false,
            })?;
            sink.push_def(def(sym, file_id, start_line, end_line))?;
            sink.push_edge(EdgeRecord {
                src_id: sym,
                target_id: enclosing.unwrap_or(doc_sym),
                properties: EdgeProperties::DefinedIn,
            })?;
            // Two surfaces, matching how code is stored: the rendered start tag
            // as the signature, the element's own text as the content.
            //
            // The split is what makes the content usable. A consumer parsing a
            // `<sql>` body needs the statement with nothing prepended —
            // `sqlparser` rejects `sql ALTER TABLE users` at token 1 — so the
            // text is stored verbatim, with no tag and no attributes mixed in.
            // Both surfaces still reach the lexical index; the store derives
            // that projection from the pair.
            sink.push_symbol_docs(SymbolDocsRecord {
                sym_id: sym,
                sig: signature(el),
                doc: el.text.clone(),
            })?;
            counts.elements += 1;
            counts.edges += 1;
        }
    }
    Ok(())
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

fn line_span(text: &str, start: usize, end: usize) -> (u32, u32) {
    let line_of = |byte: usize| -> u32 {
        let counted = text
            .get(..byte.min(text.len()))
            .map_or(0, |s| s.matches('\n').count());
        u32::try_from(counted.saturating_add(1)).unwrap_or(u32::MAX)
    };
    (line_of(start), line_of(end))
}

fn stem(relpath: &str) -> String {
    relpath
        .rsplit('/')
        .next()
        .unwrap_or(relpath)
        .rsplit_once('.')
        .map_or_else(|| relpath.to_string(), |(s, _)| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn cfg() -> kenn_config::XmlConfig {
        kenn_config::XmlConfig {
            enabled: true,
            ..Default::default()
        }
    }

    fn run(files: &[(&str, &str)]) -> (XmlCounts, TempDir) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        for (rel, body) in files {
            let p = ws.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, body).unwrap();
        }
        let building = ws.join(".kenn").join("local").join("building");
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
        let sink = BatchSink::new(writer, rt.handle().clone(), 16);
        let counts = ingest_xml(&cfg(), ws, sink).expect("ingest");
        (counts, dir)
    }

    #[test]
    fn every_element_becomes_a_node() {
        let (c, _d) = run(&[("a.xml", "<r><a/><b><c/></b></r>")]);
        assert_eq!(c.files, 1);
        assert_eq!(c.elements, 4, "r, a, b, c");
        assert_eq!(c.edges, 4, "one containment edge each");
    }

    #[test]
    fn attributes_do_not_multiply_nodes() {
        let (c, _d) = run(&[("a.xml", "<e one=\"1\" two=\"2\" three=\"3\"/>")]);
        assert_eq!(c.elements, 1, "one node whatever the attribute count");
    }

    #[test]
    fn a_malformed_file_does_not_cost_the_others() {
        let (c, _d) = run(&[("good.xml", "<r><a/></r>"), ("bad.xml", "<r><a></r>")]);
        assert_eq!(c.files, 1, "the well-formed file is still indexed");
        assert_eq!(c.failed, 1, "the failure is counted, not swallowed");
    }

    #[test]
    fn excluded_directories_are_not_walked() {
        let (c, _d) = run(&[
            ("real.xml", "<r/>"),
            ("obj/generated.xml", "<r/>"),
            ("bin/other.xml", "<r/>"),
        ]);
        assert_eq!(c.files, 1, "build output is excluded by default");
    }

    #[test]
    fn only_configured_extensions_are_claimed() {
        let (c, _d) = run(&[("a.xml", "<r/>"), ("b.xsd", "<r/>")]);
        assert_eq!(c.files, 1, ".xsd belongs to whoever configures it");
    }

    #[test]
    fn shell_hostile_text_never_reaches_a_pub_id() {
        // Both id sources are exposed: the document id is built from the file
        // path, the element id from an attribute value. Neither is under our
        // control, and a `pub_id` is handed to `kenn get` as one shell token —
        // the store writer debug-asserts that, so an unfloored id panics here
        // rather than shipping a broken id.
        let (c, _d) = run(&[("my notes.xml", "<r id=\"a b (c)\"><child/></r>")]);
        assert_eq!(c.files, 1, "a spaced path is indexed, not skipped");
        assert_eq!(c.elements, 2);
    }

    #[test]
    fn an_empty_workspace_is_not_a_failure() {
        let (c, _d) = run(&[("notes.md", "# not xml")]);
        assert_eq!(c.files, 0);
        assert_eq!(c.failed, 0);
    }
}
