//! SCIP protobuf streaming parser (section 4 of the proposal).
//!
//! Reads a `.scip` index and yields `scip::Document` messages one at a time
//! via a callback. Never materializes the parent `Index` payload — only
//! per-document state is alive at any moment.
//!
//! Why callback-based and not `Iterator`: protobuf's `CodedInputStream`
//! wraps the reader and is buffered. Holding it across iterator `.next()`
//! calls would require a self-referential struct (the CIS borrows from the
//! reader). The callback form keeps the CIS local to one call and avoids
//! unsafe self-referential shenanigans.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use protobuf::CodedInputStream;
use scip::types::{Document, Metadata};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("protobuf: {0}")]
    Protobuf(#[from] protobuf::Error),
}

/// Open `.scip` file at `path` and stream documents through `on_document`.
pub fn parse_scip_file<F: FnMut(Document) -> Result<(), ParseError>>(
    path: &Path,
    on_document: F,
) -> Result<(), ParseError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    parse_scip_stream(&mut reader, on_document)
}

/// Stream documents from any `Read` source.
pub fn parse_scip_stream<R: Read, F: FnMut(Document) -> Result<(), ParseError>>(
    reader: &mut R,
    on_document: F,
) -> Result<(), ParseError> {
    parse_scip_stream_with_metadata(reader, |_| Ok(()), on_document)
}

/// Stream both `Metadata` (field 1) and `Document` (field 2) from a SCIP
/// `Index` payload. `on_metadata` runs zero or one times depending on
/// whether the indexer emitted metadata.
pub fn parse_scip_stream_with_metadata<R, M, F>(
    reader: &mut R,
    mut on_metadata: M,
    mut on_document: F,
) -> Result<(), ParseError>
where
    R: Read,
    M: FnMut(Metadata) -> Result<(), ParseError>,
    F: FnMut(Document) -> Result<(), ParseError>,
{
    let mut cis = CodedInputStream::new(reader);
    loop {
        if cis.eof()? {
            return Ok(());
        }
        let tag = cis.read_raw_varint32()?;
        let field = tag >> 3;
        let wire_raw = tag & 0x7;
        let Some(wire_type) = wire_type_from_raw(wire_raw) else {
            return Err(ParseError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("malformed protobuf tag: {tag:#x}"),
            )));
        };
        match field {
            SCIP_INDEX_METADATA_FIELD => {
                let m: Metadata = cis.read_message()?;
                on_metadata(m)?;
            }
            SCIP_INDEX_DOCUMENTS_FIELD => {
                let d: Document = cis.read_message()?;
                on_document(d)?;
            }
            _ => cis.skip_field(wire_type)?,
        }
    }
}

/// SCIP `Index` field tags (proto schema):
/// `metadata = 1`, `documents = 2`, `external_symbols = 3`.
const SCIP_INDEX_METADATA_FIELD: u32 = 1;
const SCIP_INDEX_DOCUMENTS_FIELD: u32 = 2;

fn wire_type_from_raw(raw: u32) -> Option<protobuf::rt::WireType> {
    use protobuf::rt::WireType;
    Some(match raw {
        0 => WireType::Varint,
        1 => WireType::Fixed64,
        2 => WireType::LengthDelimited,
        3 => WireType::StartGroup,
        4 => WireType::EndGroup,
        5 => WireType::Fixed32,
        _ => return None,
    })
}

/// Convenience wrapper: collect all documents into a `Vec` (used by tests
/// where total count is expected to be small).
pub fn parse_scip_to_vec<R: Read>(reader: &mut R) -> Result<Vec<Document>, ParseError> {
    let mut docs = Vec::new();
    parse_scip_stream(reader, |d| {
        docs.push(d);
        Ok(())
    })?;
    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message;
    use scip::types::{Index, Metadata, Occurrence, SymbolInformation, ToolInfo};

    fn build_synthetic_index(num_docs: usize) -> Vec<u8> {
        let mut idx = Index::new();
        let mut md = Metadata::new();
        let mut tool = ToolInfo::new();
        tool.name = "test".into();
        tool.version = "0.0.0".into();
        md.tool_info = protobuf::MessageField::some(tool);
        md.project_root = "file:///tmp".into();
        idx.metadata = protobuf::MessageField::some(md);

        for i in 0..num_docs {
            let mut doc = Document::new();
            doc.relative_path = format!("file_{i}.rs");
            doc.language = "rust".into();
            let mut sym = SymbolInformation::new();
            sym.symbol = format!("rust-analyzer cargo test 0.0.0 m{i}/foo().");
            doc.symbols.push(sym);
            let mut occ = Occurrence::new();
            occ.range = vec![0_i32, 0, 0, 0];
            occ.symbol = format!("rust-analyzer cargo test 0.0.0 m{i}/foo().");
            doc.occurrences.push(occ);
            idx.documents.push(doc);
        }

        idx.write_to_bytes().unwrap()
    }

    #[test]
    fn streams_each_document_once() {
        let bytes = build_synthetic_index(5);
        let docs = parse_scip_to_vec(&mut &bytes[..]).unwrap();
        assert_eq!(docs.len(), 5);
        for (i, d) in docs.iter().enumerate() {
            assert_eq!(d.relative_path, format!("file_{i}.rs"));
            assert_eq!(d.symbols.len(), 1);
            assert_eq!(d.occurrences.len(), 1);
        }
    }

    #[test]
    fn callback_receives_each_document_in_order() {
        let bytes = build_synthetic_index(3);
        let mut seen: Vec<String> = Vec::new();
        parse_scip_stream(&mut &bytes[..], |d| {
            seen.push(d.relative_path);
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec!["file_0.rs", "file_1.rs", "file_2.rs"]);
    }

    #[test]
    fn handles_empty_index() {
        let mut idx = Index::new();
        idx.metadata = protobuf::MessageField::some(Metadata::new());
        let bytes = idx.write_to_bytes().unwrap();
        let docs = parse_scip_to_vec(&mut &bytes[..]).unwrap();
        assert!(docs.is_empty());
    }
}
