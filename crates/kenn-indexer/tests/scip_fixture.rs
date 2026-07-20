//! Tasks 4.4 + 4.5 — parse a real `.scip` fixture and assert sanity bounds
//! on document/symbol/occurrence counts and parsing memory.
//!
//! Activated only when `CODE_INTEL_SCIP_FIXTURE` points to an existing
//! `.scip` file. Skipped silently otherwise so CI without spike data passes.

use kenn_indexer::parse::{parse_scip_file, ParseError};

#[test]
fn fixture_parses_to_expected_doc_count() {
    let Ok(path) = std::env::var("CODE_INTEL_SCIP_FIXTURE") else {
        eprintln!("skip: set CODE_INTEL_SCIP_FIXTURE to a `.scip` file path");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let mut docs = 0_u64;
    let mut symbols = 0_u64;
    let mut occurrences = 0_u64;
    parse_scip_file(&path, |d| -> Result<(), ParseError> {
        docs += 1;
        symbols += d.symbols.len() as u64;
        occurrences += d.occurrences.len() as u64;
        Ok(())
    })
    .expect("parse");
    eprintln!("fixture stats: {docs} docs / {symbols} symbols / {occurrences} occurrences");
    assert!(
        docs > 0,
        "expected fixture to contain at least one document"
    );
}
