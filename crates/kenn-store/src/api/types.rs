//! Row, result, and option types shared by [`Reader`](crate::api::Reader)
//! and the bench / fixture harness.
//!
//! These types are backend-agnostic *shapes* — plain structs the active
//! backend's reader hydrates from its own storage.

use std::path::PathBuf;

use kenn_model::EdgeKind;

/// Errors surfaced through `Reader` and the writer.
///
/// Folded in from the previous `kenn_indexer::sink::SinkError` during
/// the storage-abstraction change: `Serde` and `Backend(String)` come
/// from there, alongside the existing `Io` variant.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("writer not initialized or already finalized")]
    NotInitialized,
    #[error("backend: {0}")]
    Backend(String),
    /// Embedding backend is still warming up (cold start or reselection
    /// after the previous remote became unreachable). Distinct so the
    /// MCP boundary can surface `EMBEDDER_STARTING` and the agent retries
    /// shortly. See `kenn_embed::EmbedError::Starting`.
    #[error("starting: {0}")]
    EmbedderStarting(String),
    /// The snapshot's persisted `schema_version` disagrees with the
    /// binary's compiled-in [`STORE_SCHEMA_VERSION`](crate::STORE_SCHEMA_VERSION).
    /// Old data cannot be safely read by the new binary; the caller must
    /// trigger a reindex (the MCP server routes this through its `Failed`
    /// lifecycle state and the existing recovery path).
    #[error("schema v{persisted}, binary expects v{expected}; reindex required")]
    SchemaMismatch { persisted: u32, expected: u32 },
}

/// Reader-pool dispatch (`Pool::conn_and_then`) requires the closure's error
/// to be `From<rusqlite::Error> + From<async_sqlite::Error>`. Both fold into
/// `Backend` (mirroring the reader's `be` mapper) so pool boundary errors and
/// query errors surface uniformly.
impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Backend(format!("sqlite: {e}"))
    }
}

impl From<async_sqlite::Error> for DbError {
    fn from(e: async_sqlite::Error) -> Self {
        DbError::Backend(format!("reader pool: {e}"))
    }
}

/// One row of the build-time `stats` table — a count at a
/// `(scope, key, subset, metric)` coordinate (build-time-stats). `subset` is
/// the lens: `internal`/`test`/`external` for entity metrics, `graph` for
/// clustering counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatRow {
    pub scope: String,
    pub key: String,
    pub subset: String,
    pub metric: String,
    pub value: i64,
}

/// Caller-tunable knobs for the active backend's writer.
#[derive(Debug, Default, Clone)]
pub struct WriterOptions {
    /// When true, defer the FULLTEXT BM25 index build (`symbols.name`,
    /// `symbol_docs.doc`) until `finalize`. The bulk of writes happen
    /// against B-tree-only schema, then BM25 indexes are constructed in
    /// one pass. Default `false` preserves the current `kenn index`
    /// behavior.
    pub defer_fulltext: bool,
    /// The committed code-vector sidecar's current **generation** dir.
    /// When set, `kenn index` reconciles cached vectors from it by
    /// fingerprint (incremental-embedding); `None` skips reconciliation,
    /// leaving every embedding null for the background embed job.
    pub vectors_dir: Option<PathBuf>,
    /// The pre-generation flat sidecar (`<vectors_root>/code/`), read as
    /// a reuse fallback so committed `pack-*.bin` files keep serving
    /// fresh clones.
    pub vectors_legacy_dir: Option<PathBuf>,
    /// The embedding model id the generation dir is keyed by — gates the
    /// reuse read (a vector from another model is never reconciled).
    /// Required for reconciliation when `vectors_dir` is set.
    pub vectors_model_id: Option<String>,
}

/// Why a `find_symbol` row matched. The variants are deliberately
/// ordered: `Exact < Prefix < Contains < Fuzzy`. Tier-ascending is
/// the primary sort key so the agent sees the best evidence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// Whole-name match (case-insensitive), via the redb
    /// `SYMBOLS_BY_NAME` key.
    Exact,
    /// `name` starts with the query, via a redb key range scan.
    Prefix,
    /// `name` contains the query as a substring — surfaced by the
    /// Lance n-gram name index.
    Contains,
    /// Surfaced by the Lance n-gram name index without a substring
    /// match, e.g. `Foo.Bar.OrderHandler.Method` for query
    /// `order handler`.
    Fuzzy,
}

impl MatchKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Prefix => "prefix",
            Self::Contains => "contains",
            Self::Fuzzy => "fuzzy",
        }
    }
}

/// One hit from `find_symbol_tiered`, carrying both the symbol row and
/// the tier that admitted it.
#[derive(Debug, Clone)]
pub struct FoundSymbolRow {
    pub symbol: SymbolRow,
    pub match_kind: MatchKind,
}

/// One hit from `search_symbols_blended`, carrying the blended composite
/// `score` rows are ranked by.
#[derive(Debug, Clone)]
pub struct BlendedSymbolRow {
    pub symbol: SymbolRow,
    pub score: f64,
}

/// One file-level-doc hit from blended search — a *file*, not a symbol.
/// Surfaced when a C# file's header/namespace comment matches the query.
#[derive(Debug, Clone)]
pub struct BlendedFileRow {
    /// The file's within-snapshot id (the `files` join key, NOT a symbol
    /// id — see `BlendedHit`).
    pub id: u32,
    pub path: String,
    pub score: f64,
}

/// One blended-search result: a symbol or a file-level-doc hit. The two
/// id spaces are independent (a file id and a symbol id can be the same
/// number), so the variant — not the id — says which dataset to resolve.
#[derive(Debug, Clone)]
pub enum BlendedHit {
    Symbol(BlendedSymbolRow),
    File(BlendedFileRow),
}

/// `SymbolRef`-shaped row directly off `symbols`.
#[derive(Debug, Clone)]
pub struct SymbolRow {
    pub id: u32,
    pub pub_id: String,
    pub language: String,
    pub pkg_id: u32,
    pub kind: String,
    pub name: String,
    pub partial: bool,
    pub nargs: i64,
    pub targs: i64,
    pub external: bool,
    pub test: bool,
    pub enclosing_sym_id: u32,
}

/// Row-level narrowing applied during traversal, BEFORE `limit` is taken — so a
/// filtered page is still a full page and the cursor stays correct.
///
/// `include_external` / `include_tests` were the only two predicates the
/// traversal honoured; `package`, `kind` and `language` were accepted by every
/// `list` command and silently dropped, so a narrowed query returned the
/// unnarrowed list and looked like an answer.
#[derive(Debug, Clone, Default)]
pub struct RowNarrow {
    pub include_external: bool,
    pub include_tests: bool,
    /// Package NAMES; resolved to ids once per traversal.
    pub packages: Option<Vec<String>>,
    /// `Kind::db_name()` values.
    pub kinds: Option<Vec<String>>,
    /// `Language::db_name()` values.
    pub languages: Option<Vec<String>>,
}

impl RowNarrow {
    /// The pre-existing two-flag form, for callers that narrow no further.
    #[must_use]
    pub fn visibility(include_external: bool, include_tests: bool) -> Self {
        Self {
            include_external,
            include_tests,
            ..Self::default()
        }
    }

    /// Whether any name-resolved predicate is set (i.e. a package lookup is
    /// worth doing).
    #[must_use]
    pub fn narrows_packages(&self) -> bool {
        self.packages.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct RankedSymbolRow {
    pub id: u32,
    pub pub_id: String,
    pub language: String,
    pub pkg_id: u32,
    pub kind: String,
    pub name: String,
    pub partial: bool,
    pub nargs: i64,
    pub targs: i64,
    pub external: bool,
    pub test: bool,
    pub enclosing_sym_id: u32,
    pub score: f64,
}

impl From<RankedSymbolRow> for SymbolRow {
    fn from(r: RankedSymbolRow) -> Self {
        Self {
            id: r.id,
            pub_id: r.pub_id,
            language: r.language,
            pkg_id: r.pkg_id,
            kind: r.kind,
            name: r.name,
            partial: r.partial,
            nargs: r.nargs,
            targs: r.targs,
            external: r.external,
            test: r.test,
            enclosing_sym_id: r.enclosing_sym_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SymbolDocsRow {
    pub sig: String,
    pub doc: String,
}

/// Full def row: file + four-tuple name range + enclosing-item line span.
#[derive(Debug, Clone)]
pub struct DefRow {
    pub file_id: u32,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// Enclosing-item extent (whole body incl. doc comment), 1-based lines;
    /// `0` when the producer supplied none. See `DefRecord`.
    pub body_start_line: u32,
    pub body_end_line: u32,
}

/// Lines-only def row: cheap projection for `path#L-L` rendering plus the
/// enclosing-item span used to read full source.
#[derive(Debug, Clone)]
pub struct DefLineRow {
    pub file_id: u32,
    pub start_line: u32,
    pub end_line: u32,
    /// Enclosing-item extent (whole body incl. doc comment), 1-based lines;
    /// `0` when absent. See `DefRecord`.
    pub body_start_line: u32,
    pub body_end_line: u32,
}

#[derive(Debug, Clone)]
pub struct PackageRow {
    pub id: u32,
    pub name: String,
    pub version: String,
    pub manager: String,
    pub external: bool,
}

/// One row from `aggregate_nodes`. Enum fields are stringified via
/// `db_name()` to match `SymbolRow` conventions.
#[derive(Debug, Clone)]
pub struct AggregateNodeRow {
    pub id: u32,
    pub kind: String,
    pub name: String,
    pub language: String,
    pub external: bool,
    pub test: bool,
    /// The node's primary definition lies under an example/sample/demo/
    /// fixture path — the third provenance flag, beside `external` and
    /// `test`. Read it; never re-derive it from paths.
    pub example: bool,
    pub anchor_id: u32,
    pub anchor_name: String,
}

/// One row from `aggregate_edges`. `min_id`/`max_id` are sorted; `kind`
/// is stringified via `EdgeKind::db_name()`.
#[derive(Debug, Clone)]
pub struct AggregateEdgeRow {
    pub src_id: u32,
    pub dst_id: u32,
    pub kind: EdgeKind,
    pub weight: u32,
}

// ── analysis tables ────────────────────────────────────────────────
// Persisted at the tail end of `kenn index`. The four tables are the
// read-side projection of `kenn_analyze::AnalysisResult`. Empty on
// snapshots indexed before this feature shipped or with
// `[index] persist_analysis = false`; readers return `vec![]` rather
// than erroring.

/// One row from `analysis_god_nodes`. Stringified `filter` (one of
/// `"live"`, `"test"`, `"external"`).
#[derive(Debug, Clone)]
pub struct AnalysisGodNodeRow {
    pub filter: String,
    pub rank: u32,
    pub short_id: u32,
    pub weighted_degree: u64,
    pub name: String,
    pub kind: String,
    pub anchor_id: u32,
    pub anchor_name: String,
}

/// One row from `analysis_flat_communities`. Flat-Louvain partition
/// over the whole snapshot, ignoring anchor boundaries.
#[derive(Debug, Clone)]
pub struct AnalysisFlatCommunityRow {
    pub community_id: u32,
    pub size: u32,
    pub total_weight: u64,
    pub cross_anchor: bool,
    pub primary_anchor_id: u32,
    pub primary_anchor_name: String,
}

/// One row from `analysis_anchored_hierarchy`. Recursive anchored
/// Louvain tree, one row per tree node.
#[derive(Debug, Clone)]
pub struct AnalysisAnchoredCommunityRow {
    pub community_id: u32,
    /// `0` is the sentinel for "no parent" (i.e. depth-0 anchor row).
    /// Callers should treat `(parent_id == 0 && depth == 0)` as the
    /// anchor-root.
    pub parent_id: u32,
    pub depth: u32,
    pub anchor_id: u32,
    pub anchor_name: String,
    pub size: u32,
    pub test_ratio: f32,
    pub test_infra: bool,
}

/// One row from `analysis_node_membership`. Per-aggregate-node lookup.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisNodeMembershipRow {
    pub short_id: u32,
    pub flat_community_id: u32,
    pub anchored_leaf_community_id: u32,
}

#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: u32,
    pub path: String,
    pub language: String,
    pub test: bool,
    pub external: bool,
}

/// One symbol's two stored search surfaces, for a bulk language-filtered scan.
///
/// The pair is what makes a cross-producer consumer possible: the signature is
/// a canonical rendering it can re-parse (an XML start tag, a statement's
/// shape), and the content is the text a parser can be handed untouched. A
/// consumer wanting one usually wants to check the other, so they arrive
/// together rather than as two round-trips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSurfaceRow {
    pub sym_id: u32,
    pub pub_id: String,
    /// Path of the file the symbol is defined in, for root filtering.
    pub path: String,
    pub sig: String,
    pub doc: String,
}

/// One symbol's stored enclosing-item extent, with the file it lives in — the
/// input for reading a symbol's own source back off disk.
///
/// Extents **nest**: a module's span contains its functions', a class's its
/// methods'. A consumer placing something found in source must therefore pick
/// the smallest containing extent, not every containing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolBodyRow {
    pub sym_id: u32,
    pub path: String,
    pub language: String,
    /// 1-based, inclusive.
    pub body_start_line: u32,
    /// 1-based, inclusive.
    pub body_end_line: u32,
    /// The symbol's own test marking — carried so a consumer can mark what it
    /// emits rather than dropping it, leaving the existing query filters to
    /// decide.
    pub test: bool,
}

/// A code symbol candidate for md→code link resolution: its short (last-segment)
/// name matched the link's target. Carries the `qualified` pub id (for
/// qualifier-drift grading) and one def's `relpath` (for locality tiebreaks).
#[derive(Debug, Clone)]
pub struct CodeSymbolHit {
    pub id: u32,
    pub qualified: String,
    pub relpath: String,
}

/// One non-exact markdown link, for the `check_links` report (the read path for
/// the `link_grade`/`link_relation` edge columns). `kind` is the edge relation
/// (`links_to` / `embeds` / `links_to_file`); `grade` is the link grade name
/// (`drifted` / `fuzzy` / `ambiguous` / `dangling`); `target` is the resolved
/// node — a markdown/code symbol `pub_id`, a code file path (for
/// `links_to_file`), or, for a dangling link, the written-but-unresolved target.
#[derive(Debug, Clone)]
pub struct LinkDiagnosticRow {
    pub src_pub_id: String,
    /// `path#L<line>` of the linking section, when known.
    pub location: Option<String>,
    pub kind: String,
    pub grade: String,
    pub target: String,
}

/// One dead-CSS finding for the `check_css` report: an unused class
/// (`orphan_class`) or a stylesheet nothing imports whose selectors are unused
/// (`orphan_stylesheet`).
#[derive(Debug, Clone)]
pub struct CssHealthRow {
    /// `orphan_class` or `orphan_stylesheet`.
    pub category: String,
    /// Public id of the class or stylesheet `module` node.
    pub pub_id: String,
    /// `path#L<line>` (class) or `path` (stylesheet), when known.
    pub location: Option<String>,
}

/// Full match counts for the `check_css` report, independent of the row cap.
#[derive(Debug, Clone, Copy, Default)]
pub struct CssHealthCounts {
    pub orphan_classes: u64,
    pub orphan_stylesheets: u64,
    /// Whether any `uses_css_class` edge exists — i.e. class-usage mining ran.
    /// When false, orphan-class detection is skipped (every class would look
    /// unused), and the caller is told `usage_sources` must be configured.
    pub usage_mining_on: bool,
}

// ── findings store ─────────────────────────────────────────────────
// A durable, provenance-bearing record of agent-derived knowledge —
// see `crate::db::findings` for the store. `parent_ids` are drawn from
// a single ID space shared with code-graph nodes: an entry is either a
// finding id (`fnd_` prefix) or a code-graph node id (`<lang>:<pub_id>`).

/// A durable, agent-derived knowledge record. Append-only — a
/// correction or deletion is a *new* finding, never an in-place edit.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// `"fnd_"` + a hyphenated `UUIDv4`.
    pub id: String,
    /// Free-form prose payload — what the finding states.
    pub text: String,
    /// Semantic-search embedding. Always `None` on a record: embeddings are
    /// *derived*, not authored, so they live in the findings vector sidecar
    /// (keyed by the text fingerprint), never in the committed `<id>.md`.
    pub embedding: Option<Vec<f32>>,
    /// Free-form, convention-driven classification labels. Also carries
    /// `supersedes:<id>` / `tombstone:<id>` lifecycle markers.
    pub tags: Vec<String>,
    /// Provenance edges — code-graph node ids and/or earlier finding ids
    /// this finding was derived from.
    pub parent_ids: Vec<String>,
    /// Creation time (UTC) — serialized as an RFC 3339 string.
    pub created_at: crate::clock::Timestamp,
}

/// One ranked hit from `search_findings`: the finding, its BM25 score,
/// and whether its code-graph evidence still resolves in the current
/// branch (read-time staleness — never persisted).
#[derive(Debug, Clone, PartialEq)]
pub struct FindingHit {
    pub finding: Finding,
    pub score: f32,
    pub stale: bool,
    /// Read-time content drift: a **file** this finding is anchored to changed
    /// content since it was anchored. Only `find_directives` computes this (it
    /// has the folded anchors + workspace root); the lexical/semantic finding
    /// search paths leave it `false`.
    pub drifted: bool,
}
