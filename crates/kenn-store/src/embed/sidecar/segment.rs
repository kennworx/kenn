//! Segment byte layout — fixed header + sorted fingerprint list +
//! int8-quantized payload. `MAX_ENTRIES` is the page-fit cap: a full
//! header (`HEADER_FIXED_LEN + count * 8`) is exactly 4096 bytes.

use crate::api::types::DbError;

use super::quant::{QuantVector, FORMAT_VERSION, QUANT_CODE_INT8};

/// File-format magic — "Kenn Vector Sidecar", format 2.
pub(super) const SEGMENT_MAGIC: [u8; 4] = *b"KVS2";
/// Fixed header prefix length — `magic`(4) + `ver_quant`(4) + `dim`(4)
/// + `count`(4).
pub(super) const HEADER_FIXED_LEN: usize = 16;
/// Per-entry payload-prefix length — scale(4); the `dim` int8 codes
/// follow.
pub(super) const ENTRY_SCALE_LEN: usize = 4;
/// Per-file cap on `count` — chosen so a full file's header
/// (`HEADER_FIXED_LEN + count * 8`) is exactly one 4 KB OS page. This
/// is a **writer protocol constant** (writers must agree on it for
/// pack-set determinism — bumping it changes the chunk boundaries on
/// the same input). Readers are count-driven and handle any value.
pub const MAX_ENTRIES: usize = (4096 - HEADER_FIXED_LEN) / 8;

/// One sidecar chunk — a set of `(fingerprint, QuantVector)` entries,
/// all of one `dim`. On disk: a fixed 16 B prefix + the sorted
/// fingerprint list (`count × 8` bytes) + the payload (entries in the
/// same fp order). See [module docs] for the byte-level layout.
#[derive(Debug, Clone)]
pub(crate) struct Segment {
    /// Vector dimension every entry carries.
    pub dim: u32,
    /// `(fingerprint, vector)` entries, sorted by fingerprint.
    pub entries: Vec<(u64, QuantVector)>,
}

impl Segment {
    /// Build a segment from `(fingerprint, QuantVector)` pairs, sorting
    /// by fingerprint and dropping duplicates (a duplicate fingerprint
    /// carries an identical vector under one model generation).
    /// Test-only — production code goes through [`super::io::append_vectors`],
    /// which performs the sort/dedup inline.
    #[cfg(test)]
    pub fn new(dim: u32, mut entries: Vec<(u64, QuantVector)>) -> Self {
        entries.sort_by_key(|(fp, _)| *fp);
        entries.dedup_by_key(|(fp, _)| *fp);
        Self { dim, entries }
    }

    /// Serialize to the on-disk byte layout.
    pub fn encode(&self) -> Result<Vec<u8>, DbError> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(DbError::Backend(format!(
                "segment has {} entries, exceeds MAX_ENTRIES={MAX_ENTRIES}",
                self.entries.len()
            )));
        }
        let dim =
            usize::try_from(self.dim).map_err(|e| DbError::Backend(format!("segment dim: {e}")))?;
        // The MAX_ENTRIES check above guarantees `len <= 510`, well
        // inside u32 range — the conversion is a const fact, not a
        // dynamic check, but funnel through `try_from` for clippy.
        let count = u32::try_from(self.entries.len())
            .map_err(|e| DbError::Backend(format!("segment count: {e}")))?;
        let mut buf = Vec::with_capacity(
            HEADER_FIXED_LEN
                + self.entries.len() * 8
                + self.entries.len() * (ENTRY_SCALE_LEN + dim),
        );
        // Fixed 16-byte header.
        buf.extend_from_slice(&SEGMENT_MAGIC);
        let ver_quant: u32 = (FORMAT_VERSION << 8) | QUANT_CODE_INT8;
        buf.extend_from_slice(&ver_quant.to_le_bytes());
        buf.extend_from_slice(&self.dim.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        // Sorted fingerprint list (`count × 8` bytes).
        for (fp, _) in &self.entries {
            buf.extend_from_slice(&fp.to_le_bytes());
        }
        // Payload — scale + codes for each entry, in the same order.
        for (_, qv) in &self.entries {
            if qv.codes.len() != dim {
                return Err(DbError::Backend(format!(
                    "segment entry has {} codes, expected dim {dim}",
                    qv.codes.len()
                )));
            }
            buf.extend_from_slice(&qv.scale.to_le_bytes());
            buf.extend(qv.codes.iter().map(|&c| c.to_le_bytes()[0]));
        }
        Ok(buf)
    }

    /// Parse a segment from its on-disk byte layout.
    pub fn decode(bytes: &[u8]) -> Result<Self, DbError> {
        let header = bytes
            .get(..HEADER_FIXED_LEN)
            .ok_or_else(|| DbError::Backend("segment truncated: no header".into()))?;
        if header.get(..4) != Some(&SEGMENT_MAGIC) {
            return Err(DbError::Backend("segment: bad magic".into()));
        }
        let ver_quant = u32::from_le_bytes(le4(header, 4)?);
        let version = ver_quant >> 8;
        let quant = ver_quant & 0xff;
        if version != FORMAT_VERSION {
            return Err(DbError::Backend(format!(
                "segment format version {version}, expected {FORMAT_VERSION}"
            )));
        }
        if quant != QUANT_CODE_INT8 {
            return Err(DbError::Backend("segment: unknown quantization".into()));
        }
        let dim_u32 = u32::from_le_bytes(le4(header, 8)?);
        let dim =
            usize::try_from(dim_u32).map_err(|e| DbError::Backend(format!("segment dim: {e}")))?;
        let count = usize::try_from(u32::from_le_bytes(le4(header, 12)?))
            .map_err(|e| DbError::Backend(format!("segment count: {e}")))?;

        let fp_len = count * 8;
        let entry_len = ENTRY_SCALE_LEN + dim;
        let expected_len = HEADER_FIXED_LEN + fp_len + count * entry_len;
        if bytes.len() != expected_len {
            return Err(DbError::Backend(format!(
                "segment body is {} bytes, expected {expected_len}",
                bytes.len()
            )));
        }
        let fp_bytes = bytes
            .get(HEADER_FIXED_LEN..HEADER_FIXED_LEN + fp_len)
            .ok_or_else(|| DbError::Backend("segment truncated: no fp list".into()))?;
        let mut fps: Vec<u64> = Vec::with_capacity(count);
        for chunk in fp_bytes.chunks_exact(8) {
            let arr: [u8; 8] = chunk
                .try_into()
                .map_err(|e| DbError::Backend(format!("segment: fp chunk size mismatch: {e}")))?;
            fps.push(u64::from_le_bytes(arr));
        }
        let payload = bytes
            .get(HEADER_FIXED_LEN + fp_len..)
            .ok_or_else(|| DbError::Backend("segment truncated: no payload".into()))?;
        let mut entries = Vec::with_capacity(count);
        for (chunk, fp) in payload.chunks_exact(entry_len).zip(fps.iter().copied()) {
            let scale = f32::from_le_bytes(le4(chunk, 0)?);
            let codes: Vec<i8> = chunk
                .get(ENTRY_SCALE_LEN..)
                .ok_or_else(|| DbError::Backend("segment entry truncated".into()))?
                .iter()
                .map(|&b| i8::from_le_bytes([b]))
                .collect();
            entries.push((fp, QuantVector { scale, codes }));
        }
        Ok(Self {
            dim: dim_u32,
            entries,
        })
    }
}

/// Read a 4-byte little-endian field at `off` within `buf`.
fn le4(buf: &[u8], off: usize) -> Result<[u8; 4], DbError> {
    buf.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| DbError::Backend("segment: truncated 4-byte field".into()))
}

#[cfg(test)]
mod tests {
    use super::super::quant::QuantVector;
    use super::{Segment, MAX_ENTRIES};

    #[test]
    fn segment_encode_decode_round_trips() {
        let entries = vec![
            (40_u64, QuantVector::quantize(&[0.1_f32, -0.2, 0.3, 0.0])),
            (10_u64, QuantVector::quantize(&[-0.4_f32, 0.5, -0.1, 0.2])),
        ];
        let seg = Segment::new(4, entries);
        // `new` sorts by fingerprint.
        assert_eq!(seg.entries.first().map(|(fp, _)| *fp), Some(10));
        let decoded = Segment::decode(&seg.encode().expect("encode")).expect("decode");
        assert_eq!(decoded.dim, 4);
        assert_eq!(decoded.entries, seg.entries);
    }

    #[test]
    fn decode_rejects_bad_magic() {
        Segment::decode(&[0_u8; 16]).unwrap_err();
    }

    #[test]
    fn decode_rejects_kvs1_magic() {
        // A file with the old KVS1 magic must be rejected — that's how
        // old sidecars become inert under KVS2 code.
        let mut bytes = vec![0u8; 16];
        bytes[..4].copy_from_slice(b"KVS1");
        Segment::decode(&bytes).unwrap_err();
    }

    #[test]
    fn encode_rejects_over_cap() {
        let entries: Vec<(u64, QuantVector)> = (0..=u64::try_from(MAX_ENTRIES).unwrap())
            .map(|i| (i, QuantVector::quantize(&[0.0_f32, 0.0])))
            .collect();
        let seg = Segment::new(2, entries);
        let err = seg.encode().expect_err("should reject over cap");
        assert!(format!("{err:?}").contains("MAX_ENTRIES"), "got {err:?}");
    }

    #[test]
    fn full_header_fits_in_one_4k_page() {
        let entries: Vec<(u64, QuantVector)> = (0..u64::try_from(MAX_ENTRIES).unwrap())
            .map(|i| (i, QuantVector::quantize(&[0.0_f32, 0.0])))
            .collect();
        let seg = Segment::new(2, entries);
        let bytes = seg.encode().expect("encode");
        let header_len = 16 + MAX_ENTRIES * 8;
        assert_eq!(header_len, 4096, "MAX_ENTRIES is the page-cap constant");
        let payload_len = MAX_ENTRIES * (4 + 2);
        assert_eq!(bytes.len(), header_len + payload_len);
    }
}
