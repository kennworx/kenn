//! Tool implementations.
//!
//! Each tool is `async fn` and runs on the rmcp runtime. Tools call
//! `state.with_db(|view| async { ... })` which acquires the lifecycle
//! lock, copies the small Ready-state metadata out, drops the guard
//! (so it doesn't span an await), then runs the closure with a
//! reference to the open `Reader`.
//!
//! Module layout:
//! - [`state`] — `ServerState` + the `ReadyView` snapshot.
//! - [`lifecycle`] — index status / reindex / watcher tools.
//! - [`query`] — symbol lookup + graph-navigation tools.
//! - [`semantic`] — semantic search + source retrieval.
//! - [`findings`] — findings store + DAG traversal tools.
//! - [`support`] — shared leaf helpers.

mod anchors;
mod contracts;
mod css;
mod documents;
mod domains;
mod findings;
mod lifecycle;
mod links;
mod packages;
mod query;
mod semantic;
mod state;
mod support;

#[cfg(test)]
mod tests;

pub use anchors::{
    check_anchors, find_directives, record_anchor, CheckAnchorsArgs, FindDirectivesArgs,
    RecordAnchorArgs,
};
pub use contracts::{list_contracts, ContractView, ImplementerView, ListContractsArgs};
pub use css::{check_css, CheckCssArgs, CheckCssResponse, CssDiagnostic};
pub use documents::{list_documents, DocumentView, ListDocumentsArgs};
pub use domains::{
    list_domains, DomainMemberView, DomainView, ListDomainsArgs, SpannedPackageView,
};
pub use findings::{
    find_predecessors, find_successors, get_finding, merge_findings, search_findings,
    store_finding, FindingDagArgs, GetFindingArgs, MergeFindingsArgs, SearchFindingsArgs,
    StoreFindingArgs,
};
pub use lifecycle::{
    get_index_status, reindex, wait_for_index, watch_start, watch_stop, GetIndexStatusArgs,
    ReindexArgs, ReindexResponse, WaitForIndexArgs, WaitForIndexResponse, WatchStartArgs,
    WatchStopArgs, WatchStopResult,
};
pub use links::{check_links, CheckLinksArgs, CheckLinksResponse, LinkDiagnostic};
pub use packages::{list_packages, CouplingView, ListPackagesArgs, PackageView};
pub use query::{
    find_at_location, find_similar, find_symbol, find_usages, get_symbol, get_workspace_overview,
    list_callees, list_callers, list_correspondences, list_implementers, list_imports,
    list_in_scope, list_module_files, list_overrides, list_usages, search_symbols, ByIdArgs,
    FindAtLocationArgs, FindSimilarArgs, FindSymbolArgs, FindUsagesArgs, GetSymbolArgs,
    GetWorkspaceOverviewArgs, ImportDirectionArg, ListImportsArgs, ListUsagesArgs,
    SearchSymbolsArgs,
};
pub use semantic::{get_source, semantic_search, GetSourceArgs, SearchScope, SemanticSearchArgs};
pub use state::{ServerState, WatchStartResult};

pub(crate) use state::ReadyView;
#[cfg(test)]
pub(crate) use support::parse_kind;
pub(crate) use support::{
    db_to_mcp, defs_for_symbol, embed_query, ensure_cursor_matches, finding_to_view, found_to_ref,
    hit_to_ref, internal, parse_language, slice_lines, split_public_id, symbol_row_to_ref,
};
