//! Diagnostic: dump high-level counts and metadata for a SCIP file.
//! Usage: `cargo run --release -p kenn-indexer --example dump_scip_summary -- <path.scip>`

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;

use anyhow::Context;
use scip::types::{Document, Metadata};

use kenn_indexer::parse::parse_scip_stream_with_metadata;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: dump_scip_summary <path.scip>")?;
    let f = File::open(&path).with_context(|| format!("open {path}"))?;
    let mut r = BufReader::new(f);
    let mut docs: u64 = 0;
    let mut occ: u64 = 0;
    let mut sym_info: u64 = 0;
    let mut by_lang: BTreeMap<String, u64> = BTreeMap::new();
    let mut sample_docs: Vec<String> = Vec::new();
    parse_scip_stream_with_metadata(
        &mut r,
        |m: Metadata| {
            eprintln!(
                "metadata: tool={} version={} project_root={} text_encoding={:?}",
                m.tool_info.name,
                m.tool_info.version,
                m.project_root,
                m.text_document_encoding.enum_value_or_default()
            );
            Ok(())
        },
        |d: Document| {
            docs += 1;
            occ += d.occurrences.len() as u64;
            sym_info += d.symbols.len() as u64;
            *by_lang.entry(d.language.clone()).or_insert(0) += 1;
            if sample_docs.len() < 3 {
                sample_docs.push(d.relative_path.clone());
            }
            Ok(())
        },
    )?;
    println!("documents: {docs}");
    println!("occurrences: {occ}");
    println!("symbol_information: {sym_info}");
    println!("by_language: {by_lang:?}");
    println!("sample paths: {sample_docs:?}");
    Ok(())
}
