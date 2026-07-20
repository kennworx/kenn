//! Common types: `SymbolRef`, `SymbolDetail`, envelopes, filters.
//!
//! These mirror the design `mcp-server/D4` shapes verbatim. Field order
//! and naming are stable agent contract.

use kenn_model::{EdgeKind, Kind, Language};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolRef {
    pub id: String,
    pub kind: Kind,
    pub language: Language,
    pub name: String,
    /// `"./path#start"` or `"./path#start-end"`. `None` for synthetic /
    /// external symbols without a source location.
    pub location: Option<String>,
    /// Resolving package's name; `""` if none.
    pub package: String,
    /// Public ID of the containing module; `""` if none.
    pub module: String,
    pub nargs: u8,
    pub targs: u8,
    pub external: bool,
    pub test: bool,
    pub partial: bool,
    /// Set by `list_usages` only — identifies which edge type matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_edge_kind: Option<EdgeKind>,
    /// Set by `list_imports(direction="both")` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<ImportDirection>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportDirection {
    Outbound,
    Inbound,
}

/// Result row for `find_symbol` — extends `SymbolRef` with the tier
/// that admitted it. `match_kind` is one of `"exact"`, `"prefix"`,
/// `"contains"`, `"fuzzy"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FoundSymbolRef {
    #[serde(flatten)]
    pub base: SymbolRef,
    pub match_kind: String,
}

/// A symbol hit from `search_symbols` — a slim projection (the full
/// symbol shape is available from `find_symbol` / the navigation tools).
/// The `id` already encodes language + qualified name, so `name` /
/// `language` / `package` / `module` and the arity flags are omitted
/// here. A null `loc` already marks an external symbol (no in-workspace
/// definition), so there is no separate `external` flag. `kind` is the
/// symbol subtype; a file hit sets the same `kind` field to `"file"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedSymbolRef {
    pub id: String,
    pub kind: Kind,
    pub loc: Option<String>,
    /// Present only when `true` — a test-only symbol. Omitted otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub test: bool,
    pub score: f64,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if signature requires &T"
)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// A file-level-doc hit from `search_symbols` — a file, not a symbol.
/// Surfaces when a C# file's header / namespace comment matches the
/// query. `kind` is always `"file"`: the same discriminant field symbol
/// hits use for their subtype, so `kind == "file"` identifies a file hit.
/// Carries only `path` (its extension implies the language) and scores.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedFileRef {
    pub kind: String,
    pub path: String,
    pub score: f64,
}

/// One `search_symbols` hit: a symbol or a file-level-doc match,
/// interleaved by score. Every hit carries a single `kind` field —
/// `"file"` for a file hit, the symbol subtype (`"class"`, …) otherwise.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SearchHitRef {
    Symbol(RankedSymbolRef),
    File(RankedFileRef),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolDetail {
    #[serde(flatten)]
    pub base: SymbolRef,
    pub sig: String,
    pub doc: String,
    pub defined_in: Option<SymbolRef>,
    /// All declaration sites. Length 1 in the common case; partial classes
    /// produce N. Rendered as `path#startLine-endLine`.
    pub defs: Vec<DefLocation>,
}

/// A declaration site rendered as `path#start_line-end_line`. Lines come
/// from the `defs` table; columns are omitted by default (see
/// `DefLocationFull` for the precise four-tuple).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefLocation {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRef {
    pub path: String,
    pub language: Language,
    pub test: bool,
    pub external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

/// One incoming reference returned by `find_usages`, tagged with the
/// resolved target it points at. `reference` is the referencing node;
/// `target` is the resolved target's `pub_id` (or, for a file/asset
/// target, its workspace-relative path). The flat, target-tagged shape
/// lets an ambiguous query's interleaved rows be grouped by `target`
/// client-side without a nested group structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageRef {
    pub reference: SymbolRef,
    pub target: String,
}

/// `find_usages` response: a flat list of target-tagged references, the
/// single-target pagination `next` cursor (null whenever the query
/// resolved to more than one target), and the multi-target truncation
/// report (`targets` traversed vs. `total_targets` matched).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FindUsagesResponse {
    pub items: Vec<UsageRef>,
    pub next: Option<String>,
    /// Distinct resolved targets actually traversed (1 when paginating).
    pub targets: u32,
    /// True when the matched-target set exceeded the cap and was trimmed.
    pub truncated: bool,
    /// Distinct targets the query matched before the cap was applied.
    pub total_targets: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleResponse<T> {
    pub item: Option<T>,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_found: Option<NotFoundHint>,
}

impl<T> SingleResponse<T> {
    #[must_use]
    pub fn found(item: T) -> Self {
        Self {
            item: Some(item),
            found: true,
            not_found: None,
        }
    }

    #[must_use]
    pub fn missing(hint: NotFoundHint) -> Self {
        Self {
            item: None,
            found: false,
            not_found: Some(hint),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct NotFoundHint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_kind: Option<Kind>,
}

/// Shared filter set on search and navigation tools.
///
/// `include_external` toggles symbols defined outside the workspace —
/// stdlib calls (Rust `Result::unwrap`), vendored crate references,
/// .NET BCL types, etc. These symbols carry `is_external = true` and
/// appear in the index for every supported language. Set to `true` to
/// surface them; defaults vary per tool (`false` for top-K search,
/// `true` for graph-walk iteration). External rows are minimal stubs
/// (no signature, no source location, kind inferred from descriptor).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Filters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Vec<Language>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<Kind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_external: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_tests: Option<bool>,
}

/// Pagination knobs.
///
/// - `page_size`: rows per response. Iteration tools default 25 / max 50;
///   top-K tools default 10 / max 30. Server clamps absurd inputs.
/// - `cursor`: opaque continuation token from a previous response's
///   `next` field. MUST be passed verbatim; do NOT parse, modify, or
///   persist across sessions. A `STALE_CURSOR` error (subcode in
///   `data.kenn_subcode`) means the index rotated — restart pagination
///   from the beginning rather than "fixing" the cursor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Pagination {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

// Iteration-tool page-size envelope.
// Agent picks rows per response; cursor walks the corpus until exhaustion.
pub const DEFAULT_PAGE: u32 = 25;
pub const MAX_PAGE: u32 = 50;

// Top-K page-size envelope. `TOP_K_MATERIALIZE` is the server-side
// cap on the ranked window: top-K tools always materialize up to this
// many rows and the cursor walks within them.
pub const DEFAULT_TOP_K_PAGE: u32 = 10;
pub const MAX_TOP_K_PAGE: u32 = 30;
pub const TOP_K_MATERIALIZE: u32 = 30;

// D7 pool ceiling — search_symbols_blended invariant: TOP_K_MATERIALIZE
// must fit under the pool ceiling or recall silently caps. If anyone
// raises TOP_K_MATERIALIZE past this, raise the pool ceiling in
// `search_symbols_blended` first.
const _: () = assert!(TOP_K_MATERIALIZE <= 256);

/// Clamp the agent's iteration-tool `page_size` to the family's bounds.
#[must_use]
pub fn clamp_page(page_size: Option<u32>) -> u32 {
    page_size.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

/// Clamp the agent's top-K `page_size` to the family's bounds. Used by
/// `search_symbols`, `search_findings`, `semantic_search`.
#[must_use]
pub fn clamp_top_k_page(page_size: Option<u32>) -> u32 {
    page_size
        .unwrap_or(DEFAULT_TOP_K_PAGE)
        .clamp(1, MAX_TOP_K_PAGE)
}

/// Per-subset counts of an entity (build-time-stats): first-party vs test vs
/// dependency/stdlib. A grand total is `internal + test + external`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubsetCounts {
    pub internal: u64,
    pub test: u64,
    pub external: u64,
}

/// Per-language graph-structure counters (from the clustering pass).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanguageGraph {
    pub nodes: u64,
    pub god_nodes: u64,
    /// Flat communities whose primary anchor is this language.
    pub communities: u64,
    pub anchors: u64,
}

/// One language's build-time stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStat {
    pub language: Language,
    pub symbols: SubsetCounts,
    pub files: SubsetCounts,
    pub defs: SubsetCounts,
    /// Graph counters; present only when the analysis pass ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<LanguageGraph>,
}

/// Package counts for one dependency manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerPackages {
    pub manager: String,
    pub internal: u64,
    pub external: u64,
}

/// Whole-graph structure summary (from the clustering pass).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphSummary {
    pub hierarchy_depth: u64,
    pub cross_anchor_communities: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub snapshot_id: String,
    pub indexed_at: String,
    /// Per-language build-time stats (counts split by subset + graph counters).
    pub languages: Vec<LanguageStat>,
    /// Package counts per dependency manager.
    pub packages_by_manager: Vec<ManagerPackages>,
    /// Whole-graph structure summary; present only when the analysis pass ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphSummary>,
    pub file_count: u64,
    pub symbol_count: u64,
    /// `None`/omitted on healthy snapshots (`symbol_count > 0`); on an
    /// empty snapshot, classifies the cause as `not-initialized` (no
    /// `kenn.toml` — suggest `kenn init`), `config-disabled` (no
    /// languages enabled in an existing `kenn.toml`), or
    /// `configured-but-empty` (enabled language(s) produced no symbols).
    /// Agents can branch on `config_hint.kind`, and `config_hint.suggestion`
    /// carries the concrete recovery action (e.g. "run `kenn init`") inline
    /// — the same prose the `EMPTY_SNAPSHOT` error gives data tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hint: Option<crate::error::ConfigHint>,
}

// ── knowledge layer ─────────────────────────────────────────────────

/// A findings-store record as returned to an MCP client.
///
/// `stale` is populated only by tools that resolve read-time staleness
/// (`search_findings`); `get_finding` returns the raw record with
/// `stale` left `false`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FindingView {
    /// `"fnd_"` + a hyphenated `UUIDv4`.
    pub id: String,
    /// Free-form prose payload — what the finding states.
    pub text: String,
    /// Free-form classification labels; also carries
    /// `supersedes:<id>` / `tombstone:<id>` lifecycle markers.
    pub tags: Vec<String>,
    /// Provenance edges — code-graph node ids and/or earlier finding
    /// ids this finding was derived from.
    pub parent_ids: Vec<String>,
    /// Creation time as an RFC 3339 (ISO 8601) UTC string.
    pub created_at: String,
    /// True when a code-graph `parent_id` no longer resolves in the
    /// current branch. Always `false` outside `search_findings`.
    #[serde(default)]
    pub stale: bool,
    /// True when a **file** this finding is anchored to changed content since it
    /// was anchored — re-read the directive before relying on it. Only
    /// `find_directives` computes this; `false` elsewhere.
    #[serde(default)]
    pub drifted: bool,
}

/// One ranked findings hit from `semantic_search` / `search_findings` —
/// the finding plus its own-corpus BM25 score.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RankedFindingView {
    #[serde(flatten)]
    pub finding: FindingView,
    pub score: f64,
}

/// One ranked code hit from `semantic_search` — a `SymbolRef` plus its
/// blended BM25 score. Scores are comparable only within the code
/// group; never cross-normalized against findings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedCodeHit {
    #[serde(flatten)]
    pub symbol: SymbolRef,
    pub score: f64,
}

/// `semantic_search` result — two independently ranked groups, each
/// tagged by its source corpus. BM25 scores are not comparable across
/// the groups.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SemanticSearchResponse {
    /// Matching code, ranked by blended BM25: symbol hits and file-level
    /// doc hits (a file hit carries `result_kind: "file"`), interleaved
    /// by score. Empty when the scope excludes code.
    pub code: Vec<SearchHitRef>,
    /// Matching findings, ranked by BM25 over finding text. Empty when
    /// the scope excludes findings.
    pub findings: Vec<RankedFindingView>,
}

/// `store_finding` result — the new finding's id plus any semantically
/// near-duplicate findings. `similar` is always empty until the
/// embedding producer lands; the field is reserved.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreFindingResponse {
    pub id: String,
    pub similar: Vec<FindingView>,
}

/// `get_source` result — the source text of a symbol's primary
/// definition plus the file path and line span it was read from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceView {
    /// Workspace-relative path of the file the def lives in.
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    /// The def's line span, verbatim from disk.
    pub text: String,
}

/// Per-server lifecycle state surfaced to MCP clients via the
/// `get_index_status` tool.
///
/// Always carries `state` (`"indexing" | "embedding" | "ready" | "disabled"
/// | "failed"`); other fields are populated based on the state. Agents should
/// branch on `state` rather than relying on individual fields being present.
/// The progression is `indexing → embedding → ready`. **Structural queries
/// (`find_symbol`, `list_callers`, …) work from `embedding` onward** — only
/// vector queries (`find_similar`, `semantic_search`) wait for `ready`; do not
/// block a structural query on `ready`. `disabled` = graph ready, no embedder,
/// so vectors will not be built (lexical-only).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexStatus {
    /// `"indexing" | "embedding" | "ready" | "disabled" | "failed"`. The
    /// remaining fields are populated based on this value. `embedding` and
    /// `disabled` both mean the code graph is ready (structural tools serve);
    /// they differ only in whether vectors are still building (`embedding`) or
    /// will never be built for lack of an embedder (`disabled`).
    pub state: String,
    /// Absolute filesystem path of the workspace the MCP server is
    /// indexing. Useful for agents to confirm they're talking to the
    /// expected workspace / worktree (especially when multiple agents
    /// run in parallel against different branches).
    pub workspace_root: String,
    /// Snapshot id (hex) — populated only when `state == "ready"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// ISO 8601 timestamp the snapshot was published — populated only
    /// when `state == "ready"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_at: Option<String>,
    /// Per-spec; reserved for the future-watcher driven update flow.
    /// Always `false` for now.
    #[serde(default)]
    pub is_stale: bool,
    /// Always `false` for now (no in-process reindex during serving in
    /// the foundation change). Kept for forward-compat.
    #[serde(default)]
    pub reindex_in_progress: bool,
    /// True when the snapshot we're reading lives in the parent
    /// worktree because the local one was unavailable.
    #[serde(default)]
    pub fallback_from_parent_worktree: bool,
    /// Pipeline progress — populated only when `state == "indexing"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<IndexStatusProgress>,
    /// Pipeline error — populated only when `state == "failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Current state of the in-process file watcher. `Off` (not running),
    /// `Idle` (running, no pending debounce), or `Debouncing` (running,
    /// an event has landed and a reindex trigger is scheduled).
    #[serde(default = "default_watcher_state")]
    pub watcher: crate::state::WatcherState,
    /// Aggregate status of the run that produced the served snapshot —
    /// `"success" | "partial"`. **Presence** is the signal: this field (and
    /// the ones below) appear exactly when the run was NOT clean — i.e. it
    /// produced at least one failed project, warning, or metric regression.
    /// The **value** is the aggregate status, which can be `"success"` for a
    /// run that succeeded but carries warnings or regressions. A clean run
    /// omits all of these, so the happy-path payload is unchanged. A
    /// fully-failed run publishes no snapshot and surfaces via
    /// `state: "failed"` instead (never here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_status: Option<String>,
    /// Per-language / per-project failure attributions from the served
    /// snapshot's run — the RAW bounded list (no `+N more` display marker).
    /// `failed_count` is the true total including dropped overflow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_projects: Vec<String>,
    /// True number of failed projects (bounded list length + overflow).
    /// Omitted when zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed_count: u64,
    /// Status-neutral producer diagnostics from the served snapshot's run
    /// (e.g. stale index-store units kept) — the RAW bounded list.
    /// `warning_count` is the true total.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// True number of warnings (bounded list length + overflow). Omitted
    /// when zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub warning_count: u64,
    /// Number of metric regressions the run recorded against the prior
    /// snapshot (surfaced by `kenn index`; the MCP self-reindex path
    /// records none). Omitted when zero. Detail (which metric, old→new) is
    /// available from `kenn status`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub regression_count: u64,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if takes &T"
)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn default_watcher_state() -> crate::state::WatcherState {
    crate::state::WatcherState::Off
}

/// Coarse-grained progress info exposed in `IndexStatus.progress`.
/// Values accumulate as the pipeline runs; `phase` tracks the latest
/// pipeline milestone (e.g. `"ingest"`, `"flush_stubs"`, `"end_run"`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexStatusProgress {
    pub phase: String,
    pub files_seen: u64,
    pub symbols_seen: u64,
    pub edges_seen: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_page_enforces_bounds() {
        assert_eq!(clamp_page(None), 25);
        assert_eq!(clamp_page(Some(0)), 1);
        assert_eq!(clamp_page(Some(50)), 50);
        assert_eq!(clamp_page(Some(10_000)), 50);
    }

    #[test]
    fn clamp_top_k_page_enforces_bounds() {
        assert_eq!(clamp_top_k_page(None), 10);
        assert_eq!(clamp_top_k_page(Some(0)), 1);
        assert_eq!(clamp_top_k_page(Some(20)), 20);
        assert_eq!(clamp_top_k_page(Some(9_999)), 30);
    }

    #[test]
    fn list_response_roundtrips_json() {
        let r: ListResponse<u32> = ListResponse {
            items: vec![1, 2, 3],
            next: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"items\":[1,2,3]"));
        assert!(!s.contains("\"total\""));
        assert!(s.contains("\"next\":null"));
    }

    #[test]
    fn single_response_omits_not_found_when_present() {
        #[derive(Serialize)]
        struct X(u32);
        let r = SingleResponse::found(X(42));
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("not_found"));
        assert!(s.contains("\"found\":true"));
    }

    #[test]
    fn single_response_includes_not_found_on_miss() {
        let r: SingleResponse<u32> = SingleResponse::missing(NotFoundHint {
            parent_id: Some("cs:Models".into()),
            parent_kind: Some(Kind::Namespace),
        });
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"found\":false"));
        assert!(s.contains("not_found"));
        assert!(s.contains("\"parent_id\":\"cs:Models\""));
    }
}

#[cfg(test)]
mod search_hit_shape {
    use super::{Kind, RankedFileRef, RankedSymbolRef, SearchHitRef};

    fn sample_symbol() -> SearchHitRef {
        SearchHitRef::Symbol(RankedSymbolRef {
            id: "cs:Acme.Orders.Handler".into(),
            kind: Kind::Class,
            loc: Some("./src/Orders.cs#10-40".into()),
            test: false,
            score: 16.5,
        })
    }

    fn sample_file() -> SearchHitRef {
        SearchHitRef::File(RankedFileRef {
            kind: "file".into(),
            path: "src/Orders.cs".into(),
            score: 7.2,
        })
    }

    #[test]
    fn wire_shape_uses_one_kind_field() {
        let sym = serde_json::to_value(sample_symbol()).unwrap();
        let file = serde_json::to_value(sample_file()).unwrap();
        println!(
            "SYMBOL HIT:\n{}\n\nFILE HIT:\n{}",
            serde_json::to_string_pretty(&sym).unwrap(),
            serde_json::to_string_pretty(&file).unwrap(),
        );
        // One discriminant field, same key on both kinds of hit.
        assert_eq!(sym["kind"], "class");
        assert_eq!(file["kind"], "file");
        // File hits carry only kind/path + the composite score.
        assert!(file.get("result_kind").is_none());
        assert!(file.get("test").is_none());
        assert!(file.get("external").is_none());
        assert!(file.get("language").is_none());
        assert!(file.get("doc_score").is_none());
        // The slim symbol hit drops `external` (null `loc` marks it) and
        // the per-arm scores (only the composite `score` remains).
        assert!(sym.get("external").is_none());
        assert!(sym.get("name_score").is_none());
        assert!(sym.get("doc_score").is_none());
        // `test: false` is omitted (present only for test-only symbols).
        assert!(sym.get("test").is_none());
    }
}
