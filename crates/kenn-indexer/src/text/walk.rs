//! Turn one text-fallback file + its chunks into `kenn_model` records: a
//! `document` node for the file, one `chunk` node per chunk (each carrying its
//! text as docs → FTS + embeddings), per-node def ranges, a `contains` edge
//! from the enclosing module to the file, and `defined_in` edges (document →
//! module, chunk → document).
//!
//! This is the markdown walker's shape (design D3) minus the heading tree and
//! link graph — text chunks are flat under their file.

use kenn_model::id::text::{chunk_id, document_id};
use kenn_model::{
    compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, ShortId,
    SymbolDocsRecord, SymbolRecord,
};

use super::discover::{DiscoveredText, ROOT_LABEL};
use super::split::Chunk;

/// Allocates text-fallback `short_id`s in the `Text` partition, with separate
/// file and symbol counters (mirroring the markdown `MarkdownIds`). One
/// instance per text ingest so ids stay unique across the whole corpus.
#[derive(Debug)]
pub struct TextIds {
    next_file: u32,
    next_symbol: u32,
}

impl Default for TextIds {
    fn default() -> Self {
        Self::new()
    }
}

impl TextIds {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_file: 1,
            next_symbol: 1,
        }
    }

    fn file_id(&mut self) -> ShortId {
        let id = compose_short_id(Language::Text, self.next_file);
        self.next_file += 1;
        id
    }

    fn symbol_id(&mut self) -> ShortId {
        let id = compose_short_id(Language::Text, self.next_symbol);
        self.next_symbol += 1;
        id
    }

    /// Mint a fresh symbol id — used by ingest to materialize the corpus root
    /// module in the same `Text` partition.
    #[must_use]
    pub fn mint_symbol(&mut self) -> ShortId {
        self.symbol_id()
    }
}

/// The records produced for one text-fallback file.
#[derive(Debug)]
pub struct TextRecords {
    pub file: FileRecord,
    pub symbols: Vec<SymbolRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    pub docs: Vec<SymbolDocsRecord>,
}

/// Walk one text-fallback file into records. `enclosing_module` is the corpus
/// root `Kind::Module` node (built once by the ingest pass); the document is a
/// `defined_in` member of it and the module `contains` the file.
#[must_use]
pub fn walk_text(
    file: &DiscoveredText,
    content: &str,
    chunks: &[Chunk],
    ids: &mut TextIds,
    enclosing_module: ShortId,
) -> TextRecords {
    let total = u32::try_from(content.lines().count())
        .unwrap_or(u32::MAX)
        .max(1);

    let file_id = ids.file_id();
    let doc_sym = ids.symbol_id();
    let mut out = TextRecords {
        file: FileRecord {
            id: file_id,
            path: file.relpath.clone(),
            language: Language::Text,
            test: false,
            external: false,
            content_hash: xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
        },
        symbols: Vec::new(),
        defs: Vec::new(),
        edges: Vec::new(),
        docs: Vec::new(),
    };

    // Document node (the file-as-node), enclosed by the corpus root module.
    out.symbols.push(symbol(
        doc_sym,
        document_id(ROOT_LABEL, &file.relpath).into_string(),
        Kind::Document,
        stem(&file.relpath),
        enclosing_module,
    ));
    out.defs.push(def(doc_sym, file_id, 1, total));
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

    // Chunk nodes, flat under the document.
    let stem = stem(&file.relpath);
    for (i, chunk) in chunks.iter().enumerate() {
        let sym = ids.symbol_id();
        out.symbols.push(symbol(
            sym,
            chunk_id(ROOT_LABEL, &file.relpath, i).into_string(),
            Kind::Chunk,
            format!("{stem}#{i}"),
            doc_sym,
        ));
        out.defs
            .push(def(sym, file_id, chunk.start_line, chunk.end_line));
        out.edges.push(EdgeRecord {
            src_id: sym,
            target_id: doc_sym,
            properties: EdgeProperties::DefinedIn,
        });
        out.docs.push(SymbolDocsRecord {
            sym_id: sym,
            sig: String::new(),
            doc: chunk.text.clone(),
        });
    }

    out
}

fn symbol(
    id: ShortId,
    pub_id: String,
    kind: Kind,
    name: String,
    enclosing_sym_id: ShortId,
) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id,
        language: Language::Text,
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

/// File stem: the last path segment with a single trailing extension removed
/// (`config/app.yaml` → `app`). A dotfile with no other extension keeps its
/// name.
fn stem(relpath: &str) -> String {
    let file = relpath.rsplit('/').next().unwrap_or(relpath);
    match file.rsplit_once('.') {
        Some((base, _ext)) if !base.is_empty() => base.to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(relpath: &str) -> DiscoveredText {
        DiscoveredText {
            abs_path: std::path::PathBuf::from(relpath),
            relpath: relpath.into(),
        }
    }

    fn chunk(text: &str, start: u32, end: u32) -> Chunk {
        Chunk {
            text: text.into(),
            start_line: start,
            end_line: end,
        }
    }

    #[test]
    fn emits_document_chunks_defs_edges_and_docs() {
        let file = disc("config/app.yaml");
        let content = "a: 1\nb: 2\nc: 3\nd: 4\n";
        let chunks = [chunk("a: 1\nb: 2\n", 1, 2), chunk("c: 3\nd: 4\n", 3, 4)];
        let mut ids = TextIds::new();
        let module = ids.mint_symbol();
        let r = walk_text(&file, content, &chunks, &mut ids, module);

        // 1 document + 2 chunks.
        assert_eq!(r.symbols.len(), 3);
        assert_eq!(r.symbols[0].kind, Kind::Document);
        assert_eq!(r.symbols[0].name, "app");
        assert_eq!(r.symbols[0].enclosing_sym_id, module);
        assert!(r.symbols[1..].iter().all(|s| s.kind == Kind::Chunk));
        assert_eq!(r.symbols[1].enclosing_sym_id, r.symbols[0].id);
        assert_eq!(r.symbols[1].pub_id, "text:workspace/config/app.yaml#0");
        assert_eq!(r.symbols[1].name, "app#0");

        // Every chunk's text is a docs record (FTS + embeddings).
        assert_eq!(r.docs.len(), 2);
        assert_eq!(r.docs[0].doc, "a: 1\nb: 2\n");

        // Document def spans the whole file; chunk defs carry their line ranges.
        assert_eq!((r.defs[0].start_line, r.defs[0].end_line), (1, 4));
        assert_eq!((r.defs[1].start_line, r.defs[1].end_line), (1, 2));
        assert_eq!((r.defs[2].start_line, r.defs[2].end_line), (3, 4));

        // module --contains--> file; document + chunks --defined_in--> parent.
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
        assert_eq!(contains, 1);
        assert_eq!(defined_in, 3); // document→module, chunk0→doc, chunk1→doc
        assert_eq!(r.edges[0].target_id, r.file.id);
    }

    #[test]
    fn stem_strips_single_extension() {
        assert_eq!(stem("config/app.yaml"), "app");
        assert_eq!(stem("data.json"), "data");
        assert_eq!(stem("Makefile"), "Makefile");
        assert_eq!(stem(".gitignore"), ".gitignore");
    }
}
