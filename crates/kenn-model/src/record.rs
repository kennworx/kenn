use serde::{Deserialize, Serialize};

use crate::{kind::Kind, language::Language};

/// Internal `u32` short id used for every cross-reference in the DB. `0` is
/// the reserved sentinel for "no reference"; auto-increment starts at `1`.
pub type ShortId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: ShortId,
    pub path: String,
    pub language: Language,
    #[serde(default)]
    pub test: bool,
    #[serde(default)]
    pub external: bool,
    pub content_hash: u64,
}

/// Package row. One per logical `(name, version)` package; the consumer
/// interns by that pair so multi-target compilations of the same library
/// collapse to one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRecord {
    pub id: ShortId,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub manager: String,
    #[serde(default)]
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub id: ShortId,
    /// Public ID = `<lang_prefix>:<key>` assembled by the consumer from the
    /// wire `key` and `MetaFrame.language`. Stored as `pub_id` because
    /// `id` is reserved for the `SurrealDB` record's primary key. NOT
    /// unique — different package versions can declare the same `pub_id`,
    /// disambiguated by `pkg`.
    #[serde(rename = "pub_id")]
    pub pub_id: String,
    pub language: Language,
    /// Owning package short id; `0` for cross-package or unknown.
    #[serde(default)]
    pub pkg_id: ShortId,
    pub kind: Kind,
    pub name: String,
    /// Direct parent (any kind). `0` for top-level.
    #[serde(default)]
    pub enclosing_sym_id: ShortId,
    #[serde(default)]
    pub partial: bool,
    // u16, not u8: real generated/large-signature code exceeds 255. A method in
    // Newtonsoft.Json overflowed a u8 here (value 257) and failed the whole
    // C# index at parse time. The DB column is already INTEGER, so this is not
    // a stored-format change.
    #[serde(default)]
    pub nargs: u16,
    #[serde(default)]
    pub targs: u16,
    /// Denormalized from `packages[pkg].external`. `pkg = 0` → `false`.
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub test: bool,
}

/// Aggregated-graph node persisted alongside symbols. One row per
/// aggregate (a class-like or module-like symbol that other symbols roll
/// up to). `short_id` is the same id as the underlying anchor symbol —
/// `aggregate_nodes` is a subset of `symbols`, keyed identically. The
/// extra anchor fields denormalize the package / path-prefix grouping so
/// readers don't need to re-resolve them per render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateNodeRecord {
    pub id: ShortId,
    pub kind: crate::kind::Kind,
    pub name: String,
    pub language: crate::language::Language,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub test: bool,
    /// The node's primary definition lies under an example/sample/demo/
    /// fixture path. Evaluated once at aggregation time (the only place
    /// that holds both the def→file map and the file paths) so no consumer
    /// has to re-derive it — a consumer that cannot see paths would
    /// otherwise have to guess, and one of them did.
    #[serde(default)]
    pub example: bool,
    /// Interned anchor id (package `short_id`, or a path-prefix id
    /// interned at aggregation time).
    pub anchor_id: u32,
    /// Human-readable anchor label (package name, first path segment, or
    /// `<unanchored>`).
    pub anchor_name: String,
}

/// Aggregated-graph undirected edge between two aggregate nodes, of one
/// specific kind. Multiple kinds between the same pair produce multiple
/// rows; weights accumulate per-pair-per-kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateEdgeRecord {
    /// Source aggregate id — the edge points **from** here (directed). The
    /// analysis normalizes to undirected on load; the atlas reads the direction.
    pub src_id: ShortId,
    /// Target aggregate id — the edge points **to** here.
    pub dst_id: ShortId,
    pub kind: crate::edge::EdgeKind,
    pub weight: u32,
}

/// Sparse documentation table — one row per symbol that has any docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolDocsRecord {
    pub sym_id: ShortId,
    #[serde(default)]
    pub sig: String,
    #[serde(default)]
    pub doc: String,
}

/// Sparse file-level documentation table — one row per file that has a
/// surviving (license-filtered) file-level comment. Mirrors
/// [`SymbolDocsRecord`]; kept separate from `files` and `symbol_docs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDocsRecord {
    pub file_id: ShortId,
    pub doc: String,
}

/// Which slice of nodes a top-N god-node ranking covers. Mirrors
/// `kenn_analyze::projection::NodeFilter` but kept lean (no `All`
/// variant) so the on-disk encoding is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GodNodeFilter {
    /// User code, not a test (`external = false && test = false`).
    Live,
    /// User test code (`external = false && test = true`).
    Test,
    /// External-package symbols (`external = true`).
    External,
}

impl GodNodeFilter {
    /// Stable string used as a key prefix / surreal-side field value.
    /// Lowercase to match the existing `EdgeKind::db_name` style.
    #[must_use]
    pub const fn db_name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Test => "test",
            Self::External => "external",
        }
    }
}

/// Top-N node by weighted degree, for one of the three filters. Each
/// `(filter, rank)` pair is unique inside a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisGodNodeRecord {
    pub filter: GodNodeFilter,
    pub rank: u32,
    pub short_id: ShortId,
    pub weighted_degree: u64,
    pub name: String,
    pub kind: crate::kind::Kind,
    pub anchor_id: ShortId,
    pub anchor_name: String,
}

/// Flat-Louvain community summary. `community_id` is dense (0..N) and
/// deterministic for a given snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisFlatCommunityRecord {
    pub community_id: u32,
    pub size: u32,
    pub total_weight: u64,
    /// True when the community contains members from more than one
    /// anchor — a cross-anchor-community diagnostic flag.
    pub cross_anchor: bool,
    /// Anchor that holds the plurality of this community's members.
    /// `0` / `<unanchored>` when ambiguous.
    pub primary_anchor_id: ShortId,
    pub primary_anchor_name: String,
}

/// One node in the recursive Louvain hierarchy. Depth 0 = anchor
/// (`parent_id` = None); deeper levels carry a `parent_id` pointing at
/// the enclosing community. The tree fits naturally as a flat table
/// because every node carries its own depth + parent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisAnchoredCommunityRecord {
    pub community_id: u32,
    /// `None` for depth-0 (anchor-root) rows.
    pub parent_id: Option<u32>,
    pub depth: u32,
    pub anchor_id: ShortId,
    pub anchor_name: String,
    pub size: u32,
    /// Fraction of members with `test = true`, in `[0.0, 1.0]`.
    pub test_ratio: f32,
    /// `test_ratio >= 0.6` — marks a test-infrastructure community.
    pub test_infra: bool,
}

/// Per-aggregate-node lookup mapping a `short_id` to its community
/// memberships. One row per aggregate node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisNodeMembershipRecord {
    pub short_id: ShortId,
    pub flat_community_id: u32,
    /// The deepest (most specific) anchored-community id this node
    /// belongs to. The full path to the anchor is reconstructed by
    /// walking `parent_id` up `AnalysisAnchoredCommunityRecord`s.
    pub anchored_leaf_community_id: u32,
}

/// Declaration site for a symbol. One row per declaration; partial classes
/// produce N rows sharing `sym_id`. Lines and columns are separate
/// columns so range-only renderings can project the line subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefRecord {
    pub sym_id: ShortId,
    pub file_id: ShortId,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// Enclosing-item extent (the whole `fn`/type/impl span incl. outer doc
    /// comment), 1-based lines; `0` when the producer supplies no extent —
    /// e.g. an old rust-analyzer with no SCIP `enclosing_range`. Lines only:
    /// `get_source` slices whole lines, so an intra-line column has no
    /// consumer. `[start,col,end,col]` above stays the NAME span (precise
    /// location + edge anchoring); this is the body span.
    pub body_start_line: u32,
    pub body_end_line: u32,
}
