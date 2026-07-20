//! The query + knowledge CLI surface — verb-grouped mirrors of the kenn-mcp
//! read/knowledge tools (`overview`, `find`, `list`, `check`, `findings`,
//! `get`). Each leaf is a thin wrapper: parse argv → build a `ServerState` →
//! call the same `kenn_mcp::tools::*` function the MCP server calls → render
//! the JSON value as TOON (default) or JSON (`--json`).
//!
//! See the `cli-query-surface` capability. The MCP server is not touched; this
//! only *calls* the tool functions.

use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;

use kenn_config::Config;
use kenn_mcp::tools::{self, ServerState};
use kenn_mcp::{Filters, McpError, McpErrorCode, Pagination, WorkspaceSource};
use kenn_model::{EdgeKind, FieldOp, Kind, Language};
use kenn_store::Layout;

use crate::exit::ExitCodes;
use crate::render::{emit, Format};

// ---------------------------------------------------------------------------
// Shared flag blocks
// ---------------------------------------------------------------------------

/// The repeatable narrowing filters (test/external are global — see `Cli`).
#[derive(Debug, Args)]
struct FilterArgs {
    /// Restrict to these symbol kinds (repeatable).
    #[arg(long)]
    kind: Vec<String>,
    /// Restrict to these languages (repeatable).
    #[arg(long)]
    language: Vec<String>,
    /// Restrict to these packages (repeatable).
    #[arg(long)]
    package: Vec<String>,
    /// Restrict to these definition files (repeatable).
    #[arg(long)]
    file: Vec<String>,
}

/// Pagination for the cursor-capable tools.
#[derive(Debug, Args)]
struct PageArgs {
    /// Rows per response.
    #[arg(long)]
    page_size: Option<u32>,
    /// Continuation cursor from a prior response's `next`.
    #[arg(long)]
    cursor: Option<String>,
    /// Drain every page (ignores `--cursor`, follows `next` to exhaustion).
    #[arg(long)]
    all: bool,
}

// ---------------------------------------------------------------------------
// Command groups
// ---------------------------------------------------------------------------

/// `kenn find` — search / resolve. A bare `<query>` runs semantic search.
///
/// Note: a single-word query equal to a subcommand name (e.g. `find similar`)
/// is parsed as that subcommand, not a query. Multi-word queries are safe.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct FindGroup {
    #[command(subcommand)]
    sub: Option<FindSub>,
    /// Semantic-search query when no subcommand is given (words joined).
    query: Vec<String>,
    /// Semantic-search scope: `code`, `findings`, or `both`.
    #[arg(long, default_value = "both")]
    scope: String,
    /// Rows per response (semantic search).
    #[arg(long)]
    page_size: Option<u32>,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum FindSub {
    /// Literal-name symbol lookup (exact→fuzzy tiers).
    Symbol {
        name: String,
        #[arg(long)]
        kind: Vec<String>,
        #[arg(long)]
        page_size: Option<u32>,
    },
    /// Natural-language ranked symbol search.
    Symbols {
        query: String,
        #[command(flatten)]
        filters: FilterArgs,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Symbols whose def range covers a `file:line`.
    AtLocation {
        file: String,
        line: u32,
        #[arg(long)]
        kind: Vec<String>,
    },
    /// Symbols nearest a given symbol's committed vector.
    Similar {
        id: String,
        #[arg(long)]
        page_size: Option<u32>,
    },
    /// Resolve a name/path/id to its incoming references.
    Usages {
        query: String,
        #[arg(long)]
        kind: Vec<String>,
        #[arg(long)]
        path: Vec<String>,
        #[arg(long)]
        package: Vec<String>,
        #[arg(long)]
        language: Vec<String>,
        #[arg(long)]
        edge_kinds: Vec<String>,
        #[command(flatten)]
        page: PageArgs,
    },
}

/// `kenn list` — enumerate edges from a known id.
#[derive(Debug, Args)]
pub struct ListGroup {
    #[command(subcommand)]
    sub: ListSub,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ListSub {
    Callers(ByIdCmd),
    Callees(ByIdCmd),
    Implementers(ByIdCmd),
    Overrides(ByIdCmd),
    Correspondences(ByIdCmd),
    InScope(ByIdCmd),
    ModuleFiles(ByIdCmd),
    /// Generalized usages (union over edge kinds).
    Usages {
        id: String,
        #[arg(long)]
        edge_kinds: Vec<String>,
        #[arg(long)]
        op_filter: Option<String>,
        #[command(flatten)]
        filters: FilterArgs,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Module dependency edges.
    Imports {
        id: String,
        /// `outbound`, `inbound`, or `both`.
        #[arg(long, default_value = "both")]
        direction: String,
        /// Import-node kind filter (distinct from the symbol-`--kind` filter).
        #[arg(long)]
        import_kind: Vec<String>,
        #[command(flatten)]
        filters: FilterArgs,
        #[command(flatten)]
        page: PageArgs,
    },
}

/// The shared `<id>` + filters + pagination shape used by most `list` leaves.
#[derive(Debug, Args)]
struct ByIdCmd {
    id: String,
    #[command(flatten)]
    filters: FilterArgs,
    #[command(flatten)]
    page: PageArgs,
}

/// `kenn check` — diagnostic sweeps.
#[derive(Debug, Args)]
pub struct CheckGroup {
    #[command(subcommand)]
    sub: CheckSub,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum CheckSub {
    /// Non-exact markdown links (drifted/fuzzy/ambiguous/dangling).
    Links {
        #[arg(long)]
        grade: Vec<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Dead CSS (orphan classes / stylesheets).
    Css {
        #[arg(long)]
        category: Vec<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Findings whose anchored files moved or vanished.
    Findings,
}

/// `kenn findings` — the knowledge store.
#[derive(Debug, Args)]
pub struct FindingsGroup {
    #[command(subcommand)]
    sub: FindingsSub,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum FindingsSub {
    /// Fetch one finding by id.
    Get { id: String },
    /// BM25 search over finding text.
    Search {
        query: String,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Store a new finding (write).
    Add {
        text: String,
        #[arg(long)]
        parent: Vec<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        anchor: Vec<String>,
    },
    /// Synthesize a new finding from several (write).
    Merge {
        ids: Vec<String>,
        #[arg(long)]
        text: String,
        #[arg(long)]
        tag: Vec<String>,
    },
    /// Directives relevant to changed paths.
    Directives {
        paths: Vec<String>,
        #[arg(long)]
        query: Option<String>,
    },
    /// Walk the derivation DAG backward.
    Predecessors { id: String },
    /// Walk the derivation DAG forward.
    Successors { id: String },
    /// Re-confirm / move / drop a finding's anchor (write).
    Touch {
        finding_id: String,
        /// `attach` (re-confirm), `detach`, or `rename`.
        #[arg(long, default_value = "attach")]
        op: String,
        /// The path, for attach/detach.
        #[arg(long)]
        anchor: Option<String>,
        /// The old path, for rename.
        #[arg(long)]
        from: Option<String>,
        /// The new path, for rename.
        #[arg(long)]
        to: Option<String>,
    },
}

/// `kenn get` — fetch one entity by exact id.
#[derive(Debug, Args)]
pub struct GetGroup {
    #[command(subcommand)]
    sub: GetSub,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum GetSub {
    /// Symbol detail (signature, docs, def site).
    Symbol { id: String },
    /// Raw source text of a symbol's definition.
    Source { id: String },
}

// ---------------------------------------------------------------------------
// Top-level grouping + dispatch
// ---------------------------------------------------------------------------

/// The query + knowledge subcommands, flattened into the top-level `Command`
/// so they read as `kenn find`, `kenn list`, … (no `query` prefix). Grouping
/// them in one enum keeps the top-level dispatch match small (CRAP §6).
#[derive(Debug, Subcommand)]
pub enum QueryCommand {
    /// Workspace overview: language, package, file, and symbol counts.
    Overview {
        #[arg(long)]
        json: bool,
    },
    /// Search / resolve symbols. A bare `<query>` runs semantic search.
    ///
    /// Note: a single-word query that matches a subcommand name (e.g.
    /// `find similar`) is parsed as that subcommand, not a query. Multi-word
    /// queries are unambiguous; use `kenn find -- <word>` to force a query.
    Find(FindGroup),
    /// Enumerate graph edges from a known symbol id.
    List(ListGroup),
    /// Diagnostic sweeps: links, css, findings.
    Check(CheckGroup),
    /// The findings knowledge store: get / search / add / merge / …
    Findings(FindingsGroup),
    /// Fetch one symbol's detail or source by exact id.
    Get(GetGroup),
}

/// Route a query subcommand to its entry fn. Small (6 arms) and fully covered
/// by tests, so it stays well under the CRAP threshold.
pub fn dispatch(cmd: QueryCommand, ctx: Ctx) -> Result<ExitCodes> {
    match cmd {
        QueryCommand::Overview { json } => run_overview(ctx, json),
        QueryCommand::Find(g) => run_find(ctx, g),
        QueryCommand::List(g) => run_list(ctx, g),
        QueryCommand::Check(g) => run_check(ctx, g),
        QueryCommand::Findings(g) => run_findings(ctx, g),
        QueryCommand::Get(g) => run_get(ctx, g),
    }
}

// ---------------------------------------------------------------------------
// Entry points (one per group)
// ---------------------------------------------------------------------------

/// Bundles the workspace context threaded into every query command.
pub struct Ctx {
    pub layout: Layout,
    pub config: Config,
    pub model_id: String,
    pub source: WorkspaceSource,
    /// Universal `--include-tests` (default false), applied to every tool
    /// call that accepts it — overrides the tool's own per-default.
    pub include_tests: bool,
    /// Universal `--include-external` (default false), same treatment.
    pub include_external: bool,
}

/// The universal test/external facets, extracted from `Ctx` and threaded into
/// the tool-arg builders. Sent explicitly on every supporting call so the
/// tool's own (varying) default never leaks through.
#[derive(Clone, Copy)]
struct Facets {
    include_tests: bool,
    include_external: bool,
}

impl Facets {
    fn of(ctx: &Ctx) -> Self {
        Self {
            include_tests: ctx.include_tests,
            include_external: ctx.include_external,
        }
    }
}

pub fn run_overview(ctx: Ctx, json: bool) -> Result<ExitCodes> {
    run_on_state(
        ctx,
        Format::from_json_flag(json),
        false,
        |state| async move {
            to_val(
                tools::get_workspace_overview(&state, tools::GetWorkspaceOverviewArgs::default())
                    .await,
            )
        },
    )
}

pub fn run_find(ctx: Ctx, g: FindGroup) -> Result<ExitCodes> {
    let fmt = Format::from_json_flag(g.json);
    let facets = Facets::of(&ctx);
    // Bare `find` (semantic) and `find symbols` (blended lexical+vector) embed
    // the query; the rest are pure lexical/graph reads.
    let embeds = matches!(g.sub, None | Some(FindSub::Symbols { .. }));
    run_on_state(ctx, fmt, embeds, |state| async move {
        find_value(&state, facets, g).await
    })
}

pub fn run_list(ctx: Ctx, g: ListGroup) -> Result<ExitCodes> {
    let fmt = Format::from_json_flag(g.json);
    let facets = Facets::of(&ctx);
    run_on_state(ctx, fmt, false, |state| async move {
        list_value(&state, facets, g.sub).await
    })
}

pub fn run_check(ctx: Ctx, g: CheckGroup) -> Result<ExitCodes> {
    let fmt = Format::from_json_flag(g.json);
    run_on_state(ctx, fmt, false, |state| async move {
        check_value(&state, g.sub).await
    })
}

pub fn run_findings(ctx: Ctx, g: FindingsGroup) -> Result<ExitCodes> {
    let fmt = Format::from_json_flag(g.json);
    // `search` embeds the query; `add`/`merge` embed the finding text; a
    // `directives --query` embeds the change description. `get`/`predecessors`/
    // `successors`/anchor-only `directives` do not.
    let embeds = matches!(
        &g.sub,
        FindingsSub::Search { .. }
            | FindingsSub::Add { .. }
            | FindingsSub::Merge { .. }
            | FindingsSub::Directives { query: Some(_), .. }
    );
    run_on_state(ctx, fmt, embeds, |state| async move {
        findings_value(&state, g.sub).await
    })
}

pub fn run_get(ctx: Ctx, g: GetGroup) -> Result<ExitCodes> {
    let fmt = Format::from_json_flag(g.json);
    run_on_state(ctx, fmt, false, |state| async move {
        match g.sub {
            GetSub::Symbol { id } => {
                to_val(tools::get_symbol(&state, &tools::GetSymbolArgs { id }).await)
            }
            GetSub::Source { id } => {
                to_val(tools::get_source(&state, &tools::GetSourceArgs { id }).await)
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Per-group dispatch → JSON value
// ---------------------------------------------------------------------------

#[expect(
    clippy::too_many_lines,
    reason = "flat dispatch: one arm per `find` subcommand, each a distinct \
              tool call. Splitting into per-arm helpers would scatter the \
              subcommand→tool mapping without reducing real complexity."
)]
async fn find_value(state: &ServerState, facets: Facets, g: FindGroup) -> Result<Value, McpError> {
    match g.sub {
        None => {
            // Bare `find <query>` → semantic search. The embedder is pre-warmed
            // in `run_on_state` (this subcommand's `embeds` flag is set).
            let query = g.query.join(" ");
            to_val(
                tools::semantic_search(
                    state,
                    &tools::SemanticSearchArgs {
                        query,
                        scope: Some(parse_enum(&g.scope, "scope")?),
                        page_size: g.page_size,
                        include_tests: Some(facets.include_tests),
                        include_external: Some(facets.include_external),
                    },
                )
                .await,
            )
        }
        Some(FindSub::Symbol {
            name,
            kind,
            page_size,
        }) => to_val(
            tools::find_symbol(
                state,
                &tools::FindSymbolArgs {
                    name,
                    kind: parse_enums(&kind, "kind")?,
                    page_size,
                    include_tests: Some(facets.include_tests),
                    include_external: Some(facets.include_external),
                },
            )
            .await,
        ),
        Some(FindSub::Symbols {
            query,
            filters,
            page,
        }) => {
            run_listing(page.all, &page, |pg| async {
                to_val(
                    tools::search_symbols(
                        state,
                        &tools::SearchSymbolsArgs {
                            query: query.clone(),
                            filters: to_filters(&filters, facets)?,
                            pagination: pg,
                        },
                    )
                    .await,
                )
            })
            .await
        }
        Some(FindSub::AtLocation { file, line, kind }) => to_val(
            tools::find_at_location(
                state,
                &tools::FindAtLocationArgs {
                    file_path: file,
                    line,
                    kind: parse_enums(&kind, "kind")?,
                },
            )
            .await,
        ),
        Some(FindSub::Similar { id, page_size }) => to_val(
            tools::find_similar(
                state,
                &tools::FindSimilarArgs {
                    id,
                    page_size,
                    include_tests: Some(facets.include_tests),
                    include_external: Some(facets.include_external),
                },
            )
            .await,
        ),
        Some(FindSub::Usages {
            query,
            kind,
            path,
            package,
            language,
            edge_kinds,
            page,
        }) => {
            let kind = parse_enums::<Kind>(&kind, "kind")?;
            let language = parse_enums::<Language>(&language, "language")?;
            let edge_kinds = parse_enums::<EdgeKind>(&edge_kinds, "edge-kinds")?;
            run_listing(page.all, &page, |pg| async {
                to_val(
                    tools::find_usages(
                        state,
                        &tools::FindUsagesArgs {
                            query: query.clone(),
                            kind: kind.clone(),
                            path: opt_vec(&path),
                            package: opt_vec(&package),
                            language: language.clone(),
                            edge_kinds: edge_kinds.clone(),
                            include_tests: Some(facets.include_tests),
                            include_external: Some(facets.include_external),
                            page_size: pg.as_ref().and_then(|p| p.page_size),
                            cursor: pg.and_then(|p| p.cursor),
                        },
                    )
                    .await,
                )
            })
            .await
        }
    }
}

async fn list_value(state: &ServerState, facets: Facets, sub: ListSub) -> Result<Value, McpError> {
    // The `<id>` + filters + pagination `list` leaves share a call shape. A
    // generic higher-order helper can't express it (an async fn as
    // `Fn(&_, &_) -> Fut` hits an HRTB limitation), so expand per-tool.
    macro_rules! by_id {
        ($tool:path, $c:expr) => {{
            let c = $c;
            run_listing(c.page.all, &c.page, |pg| async {
                to_val(
                    $tool(
                        state,
                        &tools::ByIdArgs {
                            id: c.id.clone(),
                            filters: to_filters(&c.filters, facets)?,
                            pagination: pg,
                        },
                    )
                    .await,
                )
            })
            .await
        }};
    }
    match sub {
        ListSub::Callers(c) => by_id!(tools::list_callers, c),
        ListSub::Callees(c) => by_id!(tools::list_callees, c),
        ListSub::Implementers(c) => by_id!(tools::list_implementers, c),
        ListSub::Overrides(c) => by_id!(tools::list_overrides, c),
        ListSub::Correspondences(c) => by_id!(tools::list_correspondences, c),
        ListSub::InScope(c) => by_id!(tools::list_in_scope, c),
        ListSub::ModuleFiles(c) => by_id!(tools::list_module_files, c),
        ListSub::Usages {
            id,
            edge_kinds,
            op_filter,
            filters,
            page,
        } => {
            let edge_kinds = parse_enums::<EdgeKind>(&edge_kinds, "edge-kinds")?;
            let op_filter = op_filter
                .as_deref()
                .map(|s| parse_enum::<FieldOp>(s, "op-filter"))
                .transpose()?;
            run_listing(page.all, &page, |pg| async {
                to_val(
                    tools::list_usages(
                        state,
                        &tools::ListUsagesArgs {
                            id: id.clone(),
                            edge_kinds: edge_kinds.clone(),
                            op_filter,
                            filters: to_filters(&filters, facets)?,
                            pagination: pg,
                        },
                    )
                    .await,
                )
            })
            .await
        }
        ListSub::Imports {
            id,
            direction,
            import_kind,
            filters,
            page,
        } => {
            let direction = parse_enum(&direction, "direction")?;
            run_listing(page.all, &page, |pg| async {
                to_val(
                    tools::list_imports(
                        state,
                        &tools::ListImportsArgs {
                            id: id.clone(),
                            direction,
                            kind: opt_vec(&import_kind),
                            filters: to_filters(&filters, facets)?,
                            pagination: pg,
                        },
                    )
                    .await,
                )
            })
            .await
        }
    }
}

async fn check_value(state: &ServerState, sub: CheckSub) -> Result<Value, McpError> {
    match sub {
        CheckSub::Links { grade, limit } => to_val(
            tools::check_links(
                state,
                &tools::CheckLinksArgs {
                    grade: opt_vec(&grade),
                    limit,
                },
            )
            .await,
        ),
        CheckSub::Css { category, limit } => to_val(
            tools::check_css(
                state,
                &tools::CheckCssArgs {
                    category: opt_vec(&category),
                    limit,
                },
            )
            .await,
        ),
        CheckSub::Findings => {
            to_val(tools::check_anchors(state, &tools::CheckAnchorsArgs {}).await)
        }
    }
}

async fn findings_value(state: &ServerState, sub: FindingsSub) -> Result<Value, McpError> {
    match sub {
        FindingsSub::Get { id } => {
            to_val(tools::get_finding(state, &tools::GetFindingArgs { id }).await)
        }
        FindingsSub::Search { query, page } => {
            run_listing(page.all, &page, |pg| async {
                to_val(
                    tools::search_findings(
                        state,
                        &tools::SearchFindingsArgs {
                            query: query.clone(),
                            pagination: pg,
                        },
                    )
                    .await,
                )
            })
            .await
        }
        FindingsSub::Add {
            text,
            parent,
            tag,
            anchor,
        } => to_val(
            tools::store_finding(
                state,
                &tools::StoreFindingArgs {
                    text,
                    parent_ids: opt_vec(&parent),
                    tags: opt_vec(&tag),
                    anchors: opt_vec(&anchor),
                },
            )
            .await,
        ),
        FindingsSub::Merge { ids, text, tag } => to_val(
            tools::merge_findings(
                state,
                &tools::MergeFindingsArgs {
                    ids,
                    text,
                    tags: opt_vec(&tag),
                },
            )
            .await,
        ),
        FindingsSub::Directives { paths, query } => {
            // Embedder pre-warmed in `run_on_state` when `--query` is present.
            to_val(tools::find_directives(state, &tools::FindDirectivesArgs { paths, query }).await)
        }
        FindingsSub::Predecessors { id } => {
            to_val(tools::find_predecessors(state, &tools::FindingDagArgs { id }).await)
        }
        FindingsSub::Successors { id } => {
            to_val(tools::find_successors(state, &tools::FindingDagArgs { id }).await)
        }
        FindingsSub::Touch {
            finding_id,
            op,
            anchor,
            from,
            to,
        } => to_val(
            tools::record_anchor(
                state,
                &tools::RecordAnchorArgs {
                    finding_id,
                    op,
                    anchor,
                    from,
                    to,
                },
            )
            .await,
        ),
    }
}

// ---------------------------------------------------------------------------
// Shared runner + pagination drain + conversions
// ---------------------------------------------------------------------------

/// Build the `ServerState`, bootstrap it (open the live snapshot + findings
/// store), run the async producer on a fresh runtime, and render the result.
fn run_on_state<Fut>(
    ctx: Ctx,
    fmt: Format,
    embeds: bool,
    f: impl FnOnce(Arc<ServerState>) -> Fut,
) -> Result<ExitCodes>
where
    Fut: Future<Output = Result<Value, McpError>>,
{
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let state = Arc::new(
            ServerState::with_layout_config_and_model(ctx.layout, ctx.config, ctx.model_id)
                .with_workspace_source(ctx.source),
        );
        state.bootstrap().await;
        // A one-shot process has no daemon to reuse; force the embedder's
        // backend selection + model load to finish so query-embedding tools
        // don't surface EMBEDDER_STARTING on the first (and only) call.
        if embeds {
            prewarm_embedder().await;
        }
        match f(state).await {
            Ok(v) => {
                emit(&v, fmt);
                Ok(ExitCodes::Ok)
            }
            Err(e) => {
                eprintln!("error: {e}");
                Ok(ExitCodes::Generic)
            }
        }
    })
}

/// Single page (honoring `--cursor`) unless `--all`, in which case follow
/// `next` to exhaustion and return one merged `{items, next: null}`.
async fn run_listing<F, Fut>(all: bool, page: &PageArgs, mut call: F) -> Result<Value, McpError>
where
    F: FnMut(Option<Pagination>) -> Fut,
    Fut: Future<Output = Result<Value, McpError>>,
{
    if !all {
        return call(to_pagination(page)).await;
    }
    let mut cursor: Option<String> = None;
    let mut items: Vec<Value> = Vec::new();
    loop {
        let pg = Some(Pagination {
            page_size: page.page_size,
            cursor: cursor.clone(),
        });
        let mut v = call(pg).await?;
        let Some(arr) = v.get("items").and_then(Value::as_array) else {
            // Non-list shape — return verbatim rather than fabricate a table.
            return Ok(v);
        };
        items.extend(arr.iter().cloned());
        let Some(next) = v.get("next").and_then(Value::as_str).map(str::to_owned) else {
            // Last page — keep its non-items/next fields (e.g. `find_usages`
            // truncated / total_targets, so a capped result set stays visible)
            // and just swap in the merged items.
            if let Some(obj) = v.as_object_mut() {
                obj.insert("items".to_owned(), Value::Array(items));
                obj.insert("next".to_owned(), Value::Null);
                return Ok(v);
            }
            return Ok(serde_json::json!({ "items": items, "next": Value::Null }));
        };
        cursor = Some(next);
    }
}

/// Force the shared embedder's backend selection + model load to complete, so
/// the subsequent query-embedding call doesn't surface `EMBEDDER_STARTING`.
async fn prewarm_embedder() {
    // Best-effort: a failed warm-up just means the real call may still report
    // `EMBEDDER_STARTING`. Bind (not `let _`) to keep the must-use satisfied.
    let _warm = kenn_store::shared_embedder()
        .embed_block_until_ready(&["warm"])
        .await;
}

fn to_val<T: Serialize>(r: Result<T, McpError>) -> Result<Value, McpError> {
    r.map(|v| serde_json::to_value(v).expect("tool result serializes to JSON"))
}

fn to_pagination(p: &PageArgs) -> Option<Pagination> {
    if p.page_size.is_none() && p.cursor.is_none() {
        None
    } else {
        Some(Pagination {
            page_size: p.page_size,
            cursor: p.cursor.clone(),
        })
    }
}

/// Build the tool `Filters` from the repeatable narrowing flags plus the
/// universal test/external facets. The facets are sent explicitly (always
/// `Some`) so the CLI's universal default wins over each tool's own default.
fn to_filters(f: &FilterArgs, facets: Facets) -> Result<Option<Filters>, McpError> {
    Ok(Some(Filters {
        language: parse_enums::<Language>(&f.language, "language")?,
        kind: parse_enums::<Kind>(&f.kind, "kind")?,
        package: opt_vec(&f.package),
        file: opt_vec(&f.file),
        include_external: Some(facets.include_external),
        include_tests: Some(facets.include_tests),
    }))
}

fn opt_vec(v: &[String]) -> Option<Vec<String>> {
    if v.is_empty() {
        None
    } else {
        Some(v.to_vec())
    }
}

fn parse_enum<T: serde::de::DeserializeOwned>(s: &str, what: &str) -> Result<T, McpError> {
    serde_json::from_value(Value::String(s.to_owned())).map_err(|e| {
        McpError::new(
            McpErrorCode::InvalidInput,
            format!("invalid {what} '{s}': {e}"),
        )
    })
}

fn parse_enums<T: serde::de::DeserializeOwned>(
    v: &[String],
    what: &str,
) -> Result<Option<Vec<T>>, McpError> {
    if v.is_empty() {
        return Ok(None);
    }
    v.iter()
        .map(|s| parse_enum(s, what))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}
