use std::path::Path;
use std::sync::Arc;

use kenn_config::Config;
use kenn_store::Layout;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData};

use crate::error::McpError;
use crate::tools::{
    self, ByIdArgs, CheckAnchorsArgs, CheckCssArgs, CheckLinksArgs, FindAtLocationArgs,
    FindDirectivesArgs, FindSimilarArgs, FindSymbolArgs, FindUsagesArgs, FindingDagArgs,
    GetFindingArgs, GetIndexStatusArgs, GetSourceArgs, GetSymbolArgs, GetWorkspaceOverviewArgs,
    ListImportsArgs, ListUsagesArgs, MergeFindingsArgs, RecordAnchorArgs, SearchFindingsArgs,
    SearchSymbolsArgs, SemanticSearchArgs, ServerState, StoreFindingArgs, WaitForIndexArgs,
    WatchStartArgs, WatchStopArgs,
};

use super::env::debug_env_snapshot;
use super::errors::json_result;

/// The kenn agent guide — how to use the code graph + findings/directives
/// knowledge layer. Injected into the session as the MCP server's
/// `instructions`, and printed by `kenn instructions` for the plugin's
/// `SessionStart` hook. Single source: the committed `assets/kenn-agent.md`.
pub const AGENT_GUIDE: &str = include_str!("../../assets/kenn-agent.md");

/// rmcp `ServerHandler` for kenn-mcp's tool API.
#[derive(Clone)]
pub struct KennMcpServer {
    pub(super) state: Arc<ServerState>,
    /// Used by `#[tool_handler]` to route `tools/list` and `tools/call` requests.
    #[expect(
        dead_code,
        reason = "consumed implicitly by the `#[tool_handler]` macro expansion"
    )]
    pub(super) tool_router: ToolRouter<Self>,
}

impl KennMcpServer {
    #[must_use]
    pub fn new(workspace: &Path) -> Self {
        Self::with_state(Arc::new(ServerState::new(workspace)))
    }

    /// Build the server around a caller-supplied `Arc<ServerState>` so
    /// callers can hold a clone for the background indexing task.
    #[must_use]
    pub fn with_state(state: Arc<ServerState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(vis = "pub(super)")]

impl KennMcpServer {
    // ── META ───────────────────────────────────────────────────────────────

    #[tool(
        description = "Returns the workspace's index health: a `state` (`indexing` → `embedding` → `ready`, plus `disabled` = no embedder, and `failed`), snapshot id, indexed_at timestamp, whether the read is a fallback from the parent worktree, and (when supported) freshness signals. Use as the first call in a session to confirm the index is available before issuing other queries. Structural tools (find_symbol, list_callers, …) work from `embedding` onward — only vector tools (find_similar, semantic_search) wait for `ready`; if a vector tool reports embeddings are still building, poll this until `state` is `ready` (or `disabled`)."
    )]
    async fn get_index_status(&self) -> Result<CallToolResult, ErrorData> {
        json_result(tools::get_index_status(
            &self.state,
            GetIndexStatusArgs::default(),
        ))
    }

    #[tool(
        description = "Block until the index is settled (ready with no reindex running) or the timeout elapses, then return the same payload as get_index_status plus `timed_out`. Use when get_index_status (or a data tool) reports the index is still building/indexing: call this to wait rather than polling or treating an early empty result as final. Arg: optional `timeout_ms` (default 30000, max 120000)."
    )]
    async fn wait_for_index(
        &self,
        params: Parameters<WaitForIndexArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::wait_for_index(&self.state, params.0).await)
    }

    #[tool(
        description = "Summary of the indexed workspace: distinct languages present, top-level package public ids, total file count, total symbol count, and the snapshot id. Use to bootstrap exploration of an unfamiliar repo."
    )]
    async fn get_workspace_overview(&self) -> Result<CallToolResult, ErrorData> {
        json_result(
            tools::get_workspace_overview(&self.state, GetWorkspaceOverviewArgs::default()).await,
        )
    }

    #[tool(
        description = "List markdown links that are not exact: drifted (path/qualifier stale), fuzzy, ambiguous (one of several kept candidates), or dangling (written but unresolved). Each entry gives the linking section, the edge kind (links_to / embeds / links_to_file), the grade, and the resolved-or-written target. Use to audit a docs corpus for broken or rotted links. Optional `grade` filters to specific grades; output is capped at `limit` (default 100, max 1000) with `total`/`truncated` reporting the full count."
    )]
    async fn check_links(
        &self,
        params: Parameters<CheckLinksArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::check_links(&self.state, &params.0).await)
    }

    #[tool(
        description = "List dead CSS: classes nothing uses (orphan_class) and stylesheets nothing imports whose selectors are unused (orphan_stylesheet). Each finding gives the category, the class/stylesheet node id, and its location. orphan_class needs class-usage mining (a configured [language.css] usage_sources); when that is off the category is skipped and `note` says so. Optional `category` filters to orphan_class|orphan_stylesheet; output is capped at `limit` (default 100, max 1000) with `total`/`truncated` reporting the full count."
    )]
    async fn check_css(
        &self,
        params: Parameters<CheckCssArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::check_css(&self.state, &params.0).await)
    }

    // ── SEARCH ─────────────────────────────────────────────────────────────

    #[tool(
        description = "Natural-language symbol search over names and docstrings, ranked by relevance. Each row carries a `score` and a `loc` (definition site, `null` for an external symbol with no in-workspace definition); a `kind` of `\"file\"` marks a file-level-doc hit, otherwise it is the symbol's kind. Use when you have an intent (e.g. \"order cancellation flow\"). For literal-name lookup use `find_symbol`. Returns up to the top 30 results; `pagination.page_size` sets rows per response (default 10, max 30) and the cursor walks the rest. Default `include_tests=false`."
    )]
    async fn search_symbols(
        &self,
        params: Parameters<SearchSymbolsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::search_symbols(&self.state, &params.0).await)
    }

    #[tool(
        description = "Literal-name symbol lookup with explicit match tiers: exact → prefix → case-insensitive substring → n-gram fuzzy. Each row carries `match_kind` so you see why it matched. Use when you have a literal identifier from a stack trace, task spec, or prior tool output. For natural-language search use `search_symbols`. Default `include_tests=false`, `include_external=false`, `page_size=25` (max 50)."
    )]
    async fn find_symbol(
        &self,
        params: Parameters<FindSymbolArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_symbol(&self.state, &params.0).await)
    }

    #[tool(
        description = "Exact lookup of a symbol by its public id (e.g. `cs:Models.Order`, `rs:foo::bar`). Returns SymbolDetail with signature_doc, documentation, primary_def location, partial_defs (when applicable), and the enclosing parent symbol. On miss returns `{found: false, not_found: {parent_id?, parent_kind?}}` so the agent can retry with a parent-scoped search."
    )]
    async fn get_symbol(
        &self,
        params: Parameters<GetSymbolArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::get_symbol(&self.state, &params.0).await)
    }

    #[tool(
        description = "Item-to-item semantic search: code symbols whose embedding is nearest a given symbol's own committed vector. Pass the `id` of a symbol from a prior search/find/get result. Surfaces related code the name and relationship tools miss — parallel implementations across subprojects, look-alike logic with no shared call edge. Reuses the committed vector, so it needs no embedding model — but the vectors must be built: if the given symbol has no committed embedding the call errors with `EMBEDDING_UNAVAILABLE` (run `kenn embed`), distinct from an empty result meaning no similar symbols were found. Default `include_tests=false`, `include_external=false`, `page_size=25` (max 50)."
    )]
    async fn find_similar(
        &self,
        params: Parameters<FindSimilarArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_similar(&self.state, &params.0).await)
    }

    #[tool(
        description = "Stack-trace lookup. Given a `file_path` (workspace-relative or absolute) and a 1-based line number (the editor convention; matches `get_source` output and wire `#<line>` format), returns symbols whose def_range covers that line, ordered smallest-enclosing-first (method → class → namespace). Optional `kind` array narrows to specific kinds."
    )]
    async fn find_at_location(
        &self,
        params: Parameters<FindAtLocationArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_at_location(&self.state, &params.0).await)
    }

    // ── NAVIGATE ───────────────────────────────────────────────────────────

    #[tool(
        description = "Symbols that call this symbol (incoming call edges). Default `include_tests=false`, `include_external=false` — pass `include_tests: true` to include test callers when scoping a refactor. Default `page_size=25` (max 50); cursor walks the full corpus until exhaustion."
    )]
    async fn list_callers(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_callers(&self.state, &params.0).await)
    }

    #[tool(
        description = "Symbols this symbol calls (outgoing call edges). Default `include_tests=false`."
    )]
    async fn list_callees(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_callees(&self.state, &params.0).await)
    }

    #[tool(
        description = "Concrete types implementing this trait/interface (incoming `implements` edges). Default `include_tests=false` — pass `include_tests: true` to include test mocks."
    )]
    async fn list_implementers(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_implementers(&self.state, &params.0).await)
    }

    #[tool(
        description = "Symbols that override this method (incoming `overrides` edges). Default `include_tests=false`."
    )]
    async fn list_overrides(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_overrides(&self.state, &params.0).await)
    }

    #[tool(
        description = "Generalized usages — union over multiple incoming edge kinds. Default edge_kinds = [calls, type_use, field_access, instantiates]. Each row carries `via_edge_kind` so callers can group by relation type. Use when planning a refactor scope (pass `include_tests: true` to include test sites). Default `include_tests=false`."
    )]
    async fn list_usages(
        &self,
        params: Parameters<ListUsagesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_usages(&self.state, &params.0).await)
    }

    #[tool(
        description = "One call from a name, a workspace-relative path, or a `pub_id` to its INCOMING references — fuses `find_symbol` + `list_usages` server-side. Resolution dispatches on form: a `pub_id` (e.g. `cs:Models.Order`) is used directly, a path (`src/orders/api.ts`, `assets/logo.png`) resolves to its file/asset node, a plain name goes through the name index. Each reference carries the resolved `target` it points at. Default edge set is reference-style — calls, type_use, field_access, instantiates, imports, links_to, links_to_file, embeds, uses_css_class; `edge_kinds` overrides it (`imports` is what surfaces a file/stylesheet's `<link>`/importers). Search-style: a query that matches nothing, or a real node referenced nowhere, returns an empty result, NOT an error. Pagination is single-target only: a query resolving to exactly ONE target returns a `next` cursor; an ambiguous (multi-target) query returns `next: null` with `truncated`/`total_targets` set — to get a paginating stream, narrow it to one target with a `kind`/`path`/`package` filter or pass a `pub_id`. Default `include_tests=false`, `include_external=false` — pass `include_tests: true` to include test reference sites. Default `page_size=25` (max 50)."
    )]
    async fn find_usages(
        &self,
        params: Parameters<FindUsagesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_usages(&self.state, &params.0).await)
    }

    #[tool(
        description = "Cross-language / codegen equivalents of this symbol via bidirectional `corresponds_to` edges (e.g. a TS interface that mirrors a C# DTO). Returns both inbound and outbound correspondences. Default `include_tests=false`."
    )]
    async fn list_correspondences(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_correspondences(&self.state, &params.0).await)
    }

    // ── SCOPE ──────────────────────────────────────────────────────────────

    #[tool(
        description = "Symbols defined inside this module/package. v1 returns direct children only (one hop); a transitive flag is planned. Default `include_tests=false`."
    )]
    async fn list_in_scope(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_in_scope(&self.state, &params.0).await)
    }

    #[tool(
        description = "Module dependency edges. `direction` = outbound|inbound|both. When `both`, each row carries a `direction` tag. Default `include_tests=false`."
    )]
    async fn list_imports(
        &self,
        params: Parameters<ListImportsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_imports(&self.state, &params.0).await)
    }

    #[tool(
        description = "Files contained by this module/package. Returns FileRefs (path, language, test, external) — every file in the module; test/external are flagged per row, not filtered out."
    )]
    async fn list_module_files(
        &self,
        params: Parameters<ByIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::list_module_files(&self.state, &params.0).await)
    }

    // ── KNOWLEDGE LAYER ────────────────────────────────────────────────────

    #[tool(
        description = "Unified BM25 search over code symbols and/or stored findings. `scope` = code|findings|both (default both). `page_size` is the per-corpus cap (default 10, max 30); single-shot, no cursor. Returns two independently ranked groups — `code` (blended name+doc BM25) and `findings` (BM25 over finding text); scores are NOT comparable across the groups. The code arm defaults `include_tests=false`, `include_external=false` (pass `true` to include them); the findings arm has no such dimension."
    )]
    async fn semantic_search(
        &self,
        params: Parameters<SemanticSearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::semantic_search(&self.state, &params.0).await)
    }

    #[tool(
        description = "Read the source text of a symbol's primary definition. Given a public id (e.g. `rs:foo::bar`), resolves the def's file and line span and returns the span verbatim from disk plus the path and `start_line`/`end_line`. On miss returns `{found: false}`."
    )]
    async fn get_source(
        &self,
        params: Parameters<GetSourceArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::get_source(&self.state, &params.0).await)
    }

    #[tool(
        description = "Fetch a finding by id (`fnd_…`). Returns the raw record — text, tags, parent_ids, created_at — regardless of supersede/tombstone state. On miss returns `{found: false}`."
    )]
    async fn get_finding(
        &self,
        params: Parameters<GetFindingArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::get_finding(&self.state, &params.0).await)
    }

    #[tool(
        description = "BM25 search over stored findings' text. Superseded and tombstoned findings are excluded; each surviving hit carries a `stale` flag (true when a code-graph parent_id no longer resolves in the current branch). Call this before re-investigating something — prior conclusions may already be recorded. Server materializes up to top 30 ranked findings; `pagination.page_size` controls rows per response (default 10, max 30); the cursor walks within the materialized window."
    )]
    async fn search_findings(
        &self,
        params: Parameters<SearchFindingsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::search_findings(&self.state, &params.0).await)
    }

    #[tool(
        description = "Store a finding — a durable, provenance-tracked knowledge record — and commit it immediately. `text` is the prose conclusion; `parent_ids` cite the code nodes and/or prior findings it derives from; `tags` are free strings (e.g. evidence, gotcha, plan, decision; `supersedes:<id>` / `tombstone:<id>` for lifecycle). Returns `{id, similar}`; `similar` is reserved (empty for now). Store at a stable conclusion, not after every thought."
    )]
    async fn store_finding(
        &self,
        params: Parameters<StoreFindingArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::store_finding(&self.state, &params.0).await)
    }

    #[tool(
        description = "Synthesize a new finding from several inputs and commit it immediately. The given finding `ids` are recorded as the new finding's `parent_ids`; the originals are kept as evidence. Returns the new finding's id."
    )]
    async fn merge_findings(
        &self,
        params: Parameters<MergeFindingsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::merge_findings(&self.state, &params.0).await)
    }

    #[tool(
        description = "Find directives and guides relevant to the code you're working on. Give `paths` — the changed files/dirs (e.g. from `git diff --staged`); a directive anchored to a file matches it, one anchored to a directory matches anything beneath. Results are ranked by anchor match and recency-weighted liveness, exclude superseded/tombstoned findings, and carry a `stale` flag. Optional `query` (a description of the change) adds semantic matching. Call this when starting work on an area and before committing — to surface do/don't rules (`polarity:dont`) the change might violate."
    )]
    async fn find_directives(
        &self,
        params: Parameters<FindDirectivesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_directives(&self.state, &params.0).await)
    }

    #[tool(
        description = "Report findings whose anchors (the files/dirs they apply to) no longer resolve on disk — anchors orphaned by a rename or delete. Run before a commit, then repair each with `record_anchor` (`rename` to the new path, or `detach`)."
    )]
    async fn check_anchors(
        &self,
        params: Parameters<CheckAnchorsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::check_anchors(&self.state, &params.0).await)
    }

    #[tool(
        description = "Append an event to a finding's anchor log. `op` is `attach` (the finding applies to / still applies to `anchor` — a repeat attach is the liveness signal), `detach` (no longer applies to `anchor`), or `rename` (an anchored path moved `from` → `to`). Use `attach` when a directive genuinely applied to a change; `rename`/`detach` to repair anchors flagged by `check_anchors`."
    )]
    async fn record_anchor(
        &self,
        params: Parameters<RecordAnchorArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::record_anchor(&self.state, &params.0).await)
    }

    #[tool(
        description = "Walk the derivation DAG backward from a finding id — transitively collect every id reachable through `parent_ids`, including the code-graph nodes the finding ultimately derives from. Answers \"what is this conclusion based on?\"."
    )]
    async fn find_predecessors(
        &self,
        params: Parameters<FindingDagArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_predecessors(&self.state, &params.0).await)
    }

    #[tool(
        description = "Walk the derivation DAG forward from a finding (or code-node) id — transitively collect every finding that derives from it. Answers \"what conclusions depend on this?\"."
    )]
    async fn find_successors(
        &self,
        params: Parameters<FindingDagArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        json_result(tools::find_successors(&self.state, &params.0).await)
    }

    // ── LIFECYCLE ──────────────────────────────────────────────────────────

    #[tool(
        description = "Trigger an in-process reindex of this workspace. From `ready`: runs the pipeline in the background while the server keeps serving the current snapshot, then hot-reloads on completion. From `failed`: retries the cold-start pipeline as a recovery path (no process restart). From `indexing` (cold start running) or with a reindex already in flight on this instance: coalesces, reports the in-progress run, does not start a second one. Reindex is also coalesced across multiple `kenn mcp` instances on the same workspace via the store's one-writer lock — losers wait and hot-reload the winner's snapshot. The call returns promptly; observe progress via `get_index_status` (`reindex_in_progress`, `progress`)."
    )]
    async fn reindex(&self) -> Result<CallToolResult, ErrorData> {
        json_result(tools::reindex(&self.state, tools::ReindexArgs::default()))
    }

    #[tool(
        description = "Start the in-process file watcher. The watcher observes the workspace and triggers a debounced background reindex on source/project file changes — agent does not need to call `reindex` manually. Idempotent: returns `{ started: false, debounce_ms }` if a watcher is already running. Errors when the server is not `ready` (poll `get_index_status` and retry) or when the OS rejects filesystem-watch handle creation. The agent learns about completed reindexes via the `code_updated` notification."
    )]
    async fn watch_start(&self) -> Result<CallToolResult, ErrorData> {
        json_result(tools::watch_start(&self.state, WatchStartArgs::default()))
    }

    #[tool(
        description = "Stop the in-process file watcher. Idempotent: succeeds with `{ stopped: false }` if no watcher is running. Permitted in any server state."
    )]
    async fn watch_stop(&self) -> Result<CallToolResult, ErrorData> {
        json_result(tools::watch_stop(&self.state, WatchStopArgs::default()))
    }

    #[tool(
        description = "Debug: dump the MCP subprocess's pid, cwd, and the subset of environment variables matching well-known host prefixes (CLAUDE_*, CLAUDECODE, MCP_*, AI_AGENT, XDG_*, HOME). Filtered to avoid leaking unrelated secrets. Use this to verify what env Claude Code (or any MCP host) actually passes when spawning kenn-mcp."
    )]
    async fn debug_env(&self) -> Result<CallToolResult, ErrorData> {
        json_result(Ok::<_, McpError>(debug_env_snapshot()))
    }
}

/// Serve the MCP API over stdio. Binds the transport immediately and
/// returns once the client disconnects (EOF on stdin).
///
/// Startup orchestration:
/// 1. Construct `ServerState` (initial state: `Indexing { progress:
///    None }`).
/// 2. Bind stdio.
/// 3. In the background:
///    - Decide whether to skip (live snapshot present and fresh) or
///      reindex (missing/stale).
///    - On skip: open `Reader` from `live/` and transition to `Ready`.
///    - On reindex: spawn a `tokio::task::spawn_blocking` that runs the
///      full indexing workflow, with progress events forwarded to a
///      notification-pump task that emits rmcp
///      `notifications/message` log entries.
/// 4. Tools other than `get_index_status` return `INDEX_UNAVAILABLE`
///    until the state transitions to `Ready`.
pub async fn serve_stdio(
    config: Config,
    layout: Layout,
    source: crate::state::WorkspaceSource,
    model_id: String,
) -> anyhow::Result<()> {
    use rmcp::transport::io::stdio;
    use rmcp::ServiceExt;
    let state = Arc::new(
        crate::tools::ServerState::with_layout_config_and_model(layout, config, model_id)
            .with_workspace_source(source),
    );
    let server = KennMcpServer::with_state(state.clone());
    let service = server.serve(stdio()).await?;
    let peer = service.peer().clone();

    // Layout and config now live on `state` (layout in an `ArcSwap`
    // so future rebinds can swap atomically; config in a plain field).
    // The `reindex` tool reads both from there; indexing tasks read
    // them on entry / on each poll tick.
    crate::indexing::start_background_indexing(state, peer);

    service.waiting().await?;
    Ok(())
}
