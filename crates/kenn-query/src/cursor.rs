//! Opaque cursor codec — `mcp-server` design D5,
//! `mcp-pagination-spec-alignment` requirements.
//!
//! **Contract** (per MCP 2025-11-25 pagination spec):
//! - Cursors are **opaque** to callers. The agent MUST pass them
//!   verbatim back to the same tool and MUST NOT parse, decode,
//!   modify, or persist them across sessions.
//! - Page size is **server-controlled**. The `limit` parameter on
//!   paginated tools is a server ceiling (default 25, hard cap 200),
//!   not a guaranteed page size.
//! - `nextCursor` is emitted **only** when more rows exist after the
//!   returned page. A missing cursor means the stream is exhausted.
//! - Bad cursors (malformed length, wrong shape) return JSON-RPC
//!   `-32602 Invalid params` with `data.kenn_subcode = "INVALID_CURSOR"`.
//! - Stale cursors (decoded fine but the snapshot rotated) return the
//!   same `-32602` with `data.kenn_subcode = "STALE_CURSOR"`. The
//!   correct response is to restart pagination — the old cursor is
//!   not "fixable."
//!
//! Two cursor shapes (encoding detail — agents must NOT depend on this):
//! - List cursors:   `(snapshot_id[6], last_short_id[4])` → 14 base64 chars
//! - `TopK` cursors:   `(cache_id[16], offset[4])` → 27 base64 chars
//!
//! `snapshot_id` is the first 6 bytes of `xxh64(timestamp_string)` in
//! little-endian — stable per snapshot, distinct across rebuilds. Stored
//! as a fixed-width hex string (12 chars) externally; stored as raw bytes
//! inside the cursor.
//!
//! `cache_id` is a random 16-byte UUID minted by the `ResultCache` on the
//! first call to a paginated top-K tool. The cursor carries it back
//! verbatim; the cache looks up the materialized result set.
#![expect(
    clippy::indexing_slicing,
    reason = "cursor decoder: each slice is gated by an exact-length match arm"
)]
#![expect(
    clippy::unwrap_used,
    reason = "the only unwrap in this file is `write!()` into a String, which is infallible"
)]

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use xxhash_rust::xxh64::xxh64;

use crate::error::{QueryError, QueryErrorCode};

pub const SNAPSHOT_ID_BYTES: usize = 6;
pub const LIST_CURSOR_BYTES: usize = SNAPSHOT_ID_BYTES + 4;
/// `find_usages` single-target cursor: `(snapshot[6], edge_ordinal[1],
/// last_short_id[4])` → 11 bytes. The ordinal walks the effective edge
/// kinds in order; `last_short_id` is the position within the current
/// kind's inbound stream.
pub const USAGES_CURSOR_BYTES: usize = SNAPSHOT_ID_BYTES + 1 + 4;
pub const CACHE_ID_BYTES: usize = 16;
pub const TOPK_CURSOR_BYTES: usize = CACHE_ID_BYTES + 4;

pub type CacheId = [u8; CACHE_ID_BYTES];

/// 6-byte snapshot identifier. Externally rendered as 12 lowercase hex chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotId(pub [u8; SNAPSHOT_ID_BYTES]);

impl SnapshotId {
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(SNAPSHOT_ID_BYTES * 2);
        for b in self.0 {
            use std::fmt::Write;
            write!(&mut s, "{b:02x}").unwrap();
        }
        s
    }
}

/// Derive a `SnapshotId` from the snapshot directory's ISO-8601 timestamp.
#[must_use]
pub fn snapshot_id_from_timestamp(timestamp: &str) -> SnapshotId {
    let h = xxh64(timestamp.as_bytes(), 0).to_le_bytes();
    let mut out = [0u8; SNAPSHOT_ID_BYTES];
    out.copy_from_slice(&h[..SNAPSHOT_ID_BYTES]);
    SnapshotId(out)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedCursor {
    /// Iteration tools — position-based, cursor walks the corpus.
    List {
        snapshot: SnapshotId,
        last_short_id: u32,
    },
    /// Top-K tools — cache-backed, cursor indexes into a materialized
    /// result set held in `ResultCache`.
    TopK { cache_id: CacheId, offset: u32 },
    /// `find_usages` single-target pagination — position-based over a
    /// fixed target's incoming edges. `edge_ordinal` indexes the
    /// effective edge-kind list; `last_short_id` is the last referencing
    /// node returned within that kind.
    Usages {
        snapshot: SnapshotId,
        edge_ordinal: u8,
        last_short_id: u32,
    },
}

impl DecodedCursor {
    /// The List variant carries a snapshot id directly; the `TopK` variant
    /// does NOT — its snapshot validity is enforced by the
    /// `ResultCache` (which is cleared on snapshot rotation).
    #[must_use]
    pub fn list_snapshot(&self) -> Option<SnapshotId> {
        match self {
            Self::List { snapshot, .. } | Self::Usages { snapshot, .. } => Some(*snapshot),
            Self::TopK { .. } => None,
        }
    }
}

#[must_use]
pub fn encode_list_cursor(snapshot: SnapshotId, last_short_id: u32) -> String {
    let mut buf = [0u8; LIST_CURSOR_BYTES];
    buf[..SNAPSHOT_ID_BYTES].copy_from_slice(&snapshot.0);
    buf[SNAPSHOT_ID_BYTES..].copy_from_slice(&last_short_id.to_le_bytes());
    URL_SAFE_NO_PAD.encode(buf)
}

#[must_use]
pub fn encode_usages_cursor(snapshot: SnapshotId, edge_ordinal: u8, last_short_id: u32) -> String {
    let mut buf = [0u8; USAGES_CURSOR_BYTES];
    buf[..SNAPSHOT_ID_BYTES].copy_from_slice(&snapshot.0);
    buf[SNAPSHOT_ID_BYTES] = edge_ordinal;
    buf[SNAPSHOT_ID_BYTES + 1..].copy_from_slice(&last_short_id.to_le_bytes());
    URL_SAFE_NO_PAD.encode(buf)
}

#[must_use]
pub fn encode_topk_cursor(cache_id: CacheId, offset: u32) -> String {
    let mut buf = [0u8; TOPK_CURSOR_BYTES];
    buf[..CACHE_ID_BYTES].copy_from_slice(&cache_id);
    buf[CACHE_ID_BYTES..].copy_from_slice(&offset.to_le_bytes());
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn decode_cursor(s: &str) -> Result<DecodedCursor, QueryError> {
    let bytes = URL_SAFE_NO_PAD.decode(s.as_bytes()).map_err(|e| {
        QueryError::new(
            QueryErrorCode::InvalidInput,
            format!("cursor: not valid base64: {e}"),
        )
    })?;
    match bytes.len() {
        LIST_CURSOR_BYTES => {
            let mut snap = [0u8; SNAPSHOT_ID_BYTES];
            snap.copy_from_slice(&bytes[..SNAPSHOT_ID_BYTES]);
            let mut id = [0u8; 4];
            id.copy_from_slice(&bytes[SNAPSHOT_ID_BYTES..]);
            Ok(DecodedCursor::List {
                snapshot: SnapshotId(snap),
                last_short_id: u32::from_le_bytes(id),
            })
        }
        TOPK_CURSOR_BYTES => {
            let mut cache_id = [0u8; CACHE_ID_BYTES];
            cache_id.copy_from_slice(&bytes[..CACHE_ID_BYTES]);
            let mut off = [0u8; 4];
            off.copy_from_slice(&bytes[CACHE_ID_BYTES..]);
            Ok(DecodedCursor::TopK {
                cache_id,
                offset: u32::from_le_bytes(off),
            })
        }
        USAGES_CURSOR_BYTES => {
            let mut snap = [0u8; SNAPSHOT_ID_BYTES];
            snap.copy_from_slice(&bytes[..SNAPSHOT_ID_BYTES]);
            let edge_ordinal = bytes[SNAPSHOT_ID_BYTES];
            let mut id = [0u8; 4];
            id.copy_from_slice(&bytes[SNAPSHOT_ID_BYTES + 1..]);
            Ok(DecodedCursor::Usages {
                snapshot: SnapshotId(snap),
                edge_ordinal,
                last_short_id: u32::from_le_bytes(id),
            })
        }
        other => Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!(
                "cursor: wrong length {other}, expected \
                 {LIST_CURSOR_BYTES}, {USAGES_CURSOR_BYTES}, or {TOPK_CURSOR_BYTES}"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_id_is_stable_per_timestamp() {
        let a = snapshot_id_from_timestamp("2026-05-01T15-30-00Z");
        let b = snapshot_id_from_timestamp("2026-05-01T15-30-00Z");
        let c = snapshot_id_from_timestamp("2026-05-01T15-30-01Z");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn snapshot_id_hex_is_12_chars() {
        let s = snapshot_id_from_timestamp("2026-05-01T15-30-00Z");
        let h = s.to_hex();
        assert_eq!(h.len(), 12);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn list_cursor_round_trips() {
        let snap = snapshot_id_from_timestamp("ts");
        for last in [0u32, 1, 1234, u32::MAX] {
            let s = encode_list_cursor(snap, last);
            let d = decode_cursor(&s).unwrap();
            assert_eq!(
                d,
                DecodedCursor::List {
                    snapshot: snap,
                    last_short_id: last
                }
            );
        }
    }

    #[test]
    fn usages_cursor_round_trips() {
        let snap = snapshot_id_from_timestamp("ts");
        for ordinal in [0u8, 1, 8, u8::MAX] {
            for last in [0u32, 1, 4321, u32::MAX] {
                let s = encode_usages_cursor(snap, ordinal, last);
                let d = decode_cursor(&s).unwrap();
                assert_eq!(
                    d,
                    DecodedCursor::Usages {
                        snapshot: snap,
                        edge_ordinal: ordinal,
                        last_short_id: last,
                    }
                );
            }
        }
    }

    #[test]
    fn topk_cursor_round_trips() {
        let cache_id: CacheId = [
            0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        for offset in [0u32, 1, 1234, u32::MAX] {
            let s = encode_topk_cursor(cache_id, offset);
            let d = decode_cursor(&s).unwrap();
            assert_eq!(d, DecodedCursor::TopK { cache_id, offset });
        }
    }

    /// The old 14-byte Search wire format (snap[6] + score[4] + `short_id`[4])
    /// is no longer valid — neither `LIST_CURSOR_BYTES=10` nor
    /// `TOPK_CURSOR_BYTES=20`. Old in-flight Search cursors fail to decode.
    #[test]
    fn old_14_byte_search_blob_no_longer_decodes() {
        let s = URL_SAFE_NO_PAD.encode([0u8; 14]);
        let err = decode_cursor(&s).unwrap_err();
        assert_eq!(err.code, QueryErrorCode::InvalidInput);
    }

    #[test]
    fn invalid_base64_returns_invalid_input() {
        let err = decode_cursor("!!!not-base64!!!").unwrap_err();
        assert_eq!(err.code, QueryErrorCode::InvalidInput);
    }

    #[test]
    fn wrong_length_returns_invalid_input() {
        let s = URL_SAFE_NO_PAD.encode([0u8; 5]);
        let err = decode_cursor(&s).unwrap_err();
        assert_eq!(err.code, QueryErrorCode::InvalidInput);
    }
}
