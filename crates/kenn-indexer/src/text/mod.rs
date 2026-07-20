//! Generic text-fallback producer.
//!
//! Makes user-selected non-semantic text files (yaml/json/txt/…) searchable
//! when no semantic or native producer handles them. Like markdown it is a
//! sibling producer (design D1) — discovered, split, and emitted as
//! `kenn_model` records directly — but flat (no heading tree) and single-phase
//! (no post-code link resolution).

mod discover;
mod ingest;
mod split;
mod walk;

pub use discover::{discover_text, DiscoveredText, TextDiscoverError, ROOT_LABEL};
pub use ingest::{ingest_text, TextCounts, TextIngestError};
pub use split::{split, Chunk};
pub use walk::{walk_text, TextIds, TextRecords};

/// A registered text-fallback producer: its config plus the extensions enabled
/// producers claim (computed at registration in `configure_runner`, so the
/// producer can skip files another indexer already owns without re-deriving the
/// enabled-language set).
#[derive(Debug, Clone)]
pub struct TextCorpus {
    pub config: kenn_config::TextConfig,
    pub claimed_exts: std::collections::BTreeSet<String>,
}
