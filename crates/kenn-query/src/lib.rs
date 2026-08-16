//! `kenn-query` — the read layer over a kenn snapshot.
//!
//! Every question kenn can answer about indexed code lives here: symbol lookup
//! and graph navigation, unified and semantic search, the five atlas axes
//! (packages, domains, contracts, documents, tables), the findings knowledge
//! layer, and the link/CSS checkers.
//!
//! # Two front ends, one implementation
//!
//! ```text
//!            kenn-cli                    kenn-mcp
//!         (argv → stdout)            (JSON-RPC over stdio)
//!                 \                        /
//!                  \                      /
//!                   \                    /
//!                    ▼                  ▼
//!                      kenn-query   ← you are here
//!                           │
//!                           ▼
//!                       kenn-store
//! ```
//!
//! `kenn find`, `kenn list callers`, and `kenn list tables` call the same
//! functions the MCP tools of those names call. That is not a convenience — it
//! is why the two surfaces cannot drift, and why a query proven by a CLI test is
//! proven for the agent too.
//!
//! # The rule
//!
//! **This crate may not depend on a transport.** No `rmcp`, no JSON-RPC codes,
//! no argv parsing. A query returns a [`types`] value or a [`QueryError`]; what
//! a front end does with either is the front end's business — `kenn-mcp` maps
//! the error onto JSON-RPC's numeric space in its own `server/errors.rs`, and
//! the CLI renders the same error's stable string code.
//!
//! The rule is mechanically enforced: the dependency is absent from
//! `Cargo.toml`, so reaching for it does not compile.
//!
//! # What a host supplies
//!
//! A query takes a [`QueryCtx`] — an open connection, the snapshot id, the
//! workspace config, and the [`QueryCaches`] the host owns across calls. It
//! deliberately cannot see a lifecycle, a file watcher, or an MCP peer: those
//! are facts about a running daemon, not about the code being queried. Deciding
//! whether a snapshot is servable at all stays with the host, upstream of every
//! function here.

mod anchors;
mod contracts;
mod css;
mod ctx;
mod documents;
mod domains;
mod findings;
mod links;
mod packages;
mod semantic;
mod support;
mod symbols;
mod tables;

pub mod cursor;
pub mod error;
pub mod result_cache;
pub mod types;

pub use anchors::{
    check_anchors, find_directives, record_anchor, CheckAnchorsArgs, FindDirectivesArgs,
    RecordAnchorArgs,
};
pub use contracts::{list_contracts, ContractView, ImplementerView, ListContractsArgs};
pub use css::{check_css, CheckCssArgs, CheckCssResponse, CssDiagnostic};
pub use ctx::{FindingsRead, FindingsWrite, QueryCaches, QueryCtx};
pub use documents::{list_documents, DocumentView, ListDocumentsArgs};
pub use domains::{
    list_domains, DomainMemberView, DomainView, ListDomainsArgs, SpannedPackageView,
};
pub use error::{ConfigHint, QueryError, QueryErrorCode};
pub use findings::{
    find_predecessors, find_successors, get_finding, merge_findings, search_findings,
    store_finding, FindingDagArgs, GetFindingArgs, MergeFindingsArgs, SearchFindingsArgs,
    StoreFindingArgs,
};
pub use links::{check_links, CheckLinksArgs, CheckLinksResponse, LinkDiagnostic};
pub use packages::{list_packages, CouplingView, ListPackagesArgs, PackageView};
pub use semantic::{get_source, semantic_search, GetSourceArgs, SearchScope, SemanticSearchArgs};
pub use symbols::{
    find_at_location, find_similar, find_symbol, find_usages, get_symbol, get_workspace_overview,
    list_callees, list_callers, list_correspondences, list_implementers, list_imports,
    list_in_scope, list_module_files, list_overrides, list_usages, search_symbols, ByIdArgs,
    FindAtLocationArgs, FindSimilarArgs, FindSymbolArgs, FindUsagesArgs, GetSymbolArgs,
    GetWorkspaceOverviewArgs, ImportDirectionArg, ListImportsArgs, ListUsagesArgs,
    SearchSymbolsArgs,
};
pub use tables::{list_tables, ListTablesArgs, TableRefView, TableView};

pub use cursor::{
    decode_cursor, encode_list_cursor, encode_topk_cursor, encode_usages_cursor,
    snapshot_id_from_timestamp, CacheId, DecodedCursor, SnapshotId,
};
pub use types::{
    EmbedStage, FileRef, Filters, FindUsagesResponse, FindingView, ListResponse, Pagination,
    RankedCodeHit, RankedFindingView, SemanticSearchResponse, SingleResponse, SourceView,
    StoreFindingResponse, SymbolDetail, SymbolRef, UsageRef, WorkspaceInfo,
};

/// Wrap any displayable error as `INTERNAL_ERROR`.
///
/// Public because a host needs it too: `kenn-mcp` reports a poisoned lifecycle
/// lock and a failed connection checkout the same way a query reports a failed
/// read, and there is no reason for two spellings of that.
pub use support::internal;

pub(crate) use support::{
    db_to_mcp, defs_for_symbol, embed_query, ensure_cursor_matches, finding_to_view, found_to_ref,
    hit_to_ref, parse_language, slice_lines, split_public_id, symbol_row_to_ref,
};
