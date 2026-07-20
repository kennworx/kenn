//! Text-fallback ingest: the single-phase sibling-producer pass.
//!
//! Unlike markdown/css/html there is no post-code barrier — text chunks have no
//! link graph, so discovery → split → walk → write happens in one pass and the
//! sink is finished here. Emits one corpus root `Kind::Module`, then a
//! `document` + `chunk` nodes per file.

use std::collections::BTreeSet;
use std::path::Path;

use kenn_config::TextConfig;
use kenn_model::id::text::module_id;
use kenn_model::{Kind, Language, ShortId, SymbolRecord};
use kenn_store::api::DbError;

use super::discover::{discover_text, TextDiscoverError, ROOT_LABEL};
use super::split::split;
use super::walk::{walk_text, TextIds};
use crate::sink::BatchSink;

#[derive(Debug, thiserror::Error)]
pub enum TextIngestError {
    #[error(transparent)]
    Discover(#[from] TextDiscoverError),
    #[error("store append failed: {0}")]
    Store(#[from] DbError),
}

/// Record counts produced by the text ingest, for the run report (so
/// `kenn status` meta and the count-regression check see text, not zeros).
#[derive(Debug, Default, Clone, Copy)]
pub struct TextCounts {
    pub files: u64,
    pub symbols: u64,
    pub defs: u64,
    pub edges: u64,
}

/// Discover + split + walk every configured text file, emit its nodes through
/// `sink`, and finish it. `claimed_exts` are the extensions enabled producers
/// own (skipped, no double-indexing).
pub fn ingest_text(
    config: &TextConfig,
    workspace_root: &Path,
    claimed_exts: &BTreeSet<String>,
    mut sink: BatchSink,
) -> Result<TextCounts, TextIngestError> {
    let discovered = discover_text(config, workspace_root, claimed_exts)?;
    let mut counts = TextCounts::default();
    if discovered.is_empty() {
        sink.finish()?;
        return Ok(counts);
    }

    let mut ids = TextIds::new();
    // Corpus root module id — allocated up front so it precedes the file/node
    // ids, but only *written* once the first file actually yields chunks, so an
    // all-empty / all-unreadable corpus leaves no orphan module behind.
    let module = ids.mint_symbol();
    let mut module_written = false;

    for file in &discovered {
        let Ok(content) = std::fs::read_to_string(&file.abs_path) else {
            tracing::warn!(
                target: "kenn_indexer::text",
                path = %file.abs_path.display(),
                "unreadable text file, skipped"
            );
            continue;
        };
        let chunks = split(&content, config.target_chars, config.overlap_chars);
        if chunks.is_empty() {
            continue; // empty / whitespace-only file
        }
        if !module_written {
            sink.push_symbol(module_symbol(module))?;
            counts.symbols += 1;
            module_written = true;
        }
        let records = walk_text(file, &content, &chunks, &mut ids, module);
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

    sink.finish()?;
    Ok(counts)
}

/// The corpus root `Kind::Module` node (`text:workspace`), enclosing nothing.
fn module_symbol(id: ShortId) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: module_id(ROOT_LABEL).into_string(),
        language: Language::Text,
        pkg_id: 0,
        kind: Kind::Module,
        name: ROOT_LABEL.to_string(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn text_cfg(include: &[&str]) -> TextConfig {
        TextConfig {
            enabled: true,
            include: include.iter().map(|s| (*s).to_string()).collect(),
            excludes: Vec::new(),
            target_chars: 1000,
            overlap_chars: 150,
        }
    }

    #[test]
    fn ingests_text_records_into_the_store() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::create_dir_all(ws.join("config")).unwrap();
        fs::write(ws.join("config/app.yaml"), "a: 1\nb: 2\n").unwrap();
        fs::write(ws.join("data.json"), "{\"k\": 1}\n").unwrap();
        // A rust file under the (broad) include glob must NOT be indexed when
        // `rs` is a claimed extension (scenario 3.2, at the ingest level).
        fs::write(ws.join("lib.rs"), "fn main() {}\n").unwrap();

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
        let claimed: BTreeSet<String> = ["rs".to_string()].into_iter().collect();
        let counts = ingest_text(&text_cfg(&["**/*"]), ws, &claimed, sink).expect("ingest");

        // Only the two config files were indexed — lib.rs was skipped.
        assert_eq!(counts.files, 2);
        // 1 root module + per file (1 document + ≥1 chunk).
        assert!(counts.symbols >= 5, "symbols = {}", counts.symbols);
        assert!(counts.defs >= 4 && counts.edges >= 4);
    }

    #[test]
    fn all_whitespace_corpus_writes_no_orphan_module() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // Included, but whitespace-only → no chunks → the root module must not
        // be written (no orphan; counts stay zero).
        fs::write(ws.join("blank.log"), "   \n\n\t\n").unwrap();
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
        let counts =
            ingest_text(&text_cfg(&["**/*.log"]), ws, &BTreeSet::new(), sink).expect("ingest");
        assert_eq!(counts.files, 0);
        assert_eq!(counts.symbols, 0, "no orphan root module");
    }

    #[test]
    fn empty_include_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::write(ws.join("app.yaml"), "a: 1\n").unwrap();
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
        let counts = ingest_text(&text_cfg(&[]), ws, &BTreeSet::new(), sink).expect("ingest");
        assert_eq!(counts.files, 0);
        assert_eq!(counts.symbols, 0);
    }
}
