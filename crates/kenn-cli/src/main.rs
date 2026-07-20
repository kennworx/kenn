//! `kenn` CLI binary. Subcommands: `init`, `index`, `status`, `rollback`.
//!
//! See `index-store-cli` capability for the full surface.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use clap_complete::Shell;

mod cmd_cc_hook;
mod cmd_completions;
mod cmd_docker_cache;
mod cmd_doctor;
mod cmd_embed;
mod cmd_gc;
mod cmd_index;
mod cmd_init;
mod cmd_mcp;
mod cmd_query;
mod cmd_rollback;
mod cmd_server;
mod cmd_status;
mod cmd_update;
mod cmd_visualize;
mod exit;
mod init;
mod render;

use exit::ExitCodes;

#[derive(Debug, Parser)]
#[command(name = "kenn", about = "Code structure indexer + store")]
struct Cli {
    /// Workspace root. Defaults to `git rev-parse --show-toplevel`, falling
    /// back to the current working directory.
    #[arg(short = 'w', long, global = true)]
    workspace: Option<PathBuf>,

    /// Configuration file. Defaults to `<workspace>/kenn.toml`.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Include symbols defined in test files. Global — accepted on every
    /// command and honored by the tools that filter by test-ness (symbol
    /// search + graph navigation). Default off. Bare `--include-tests` = true;
    /// explicit form `--include-tests=true|false`.
    #[arg(
        long,
        global = true,
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
        require_equals = true
    )]
    include_tests: bool,

    /// Include symbols defined outside the workspace (stdlib, vendored).
    /// Global — accepted on every command, honored by the filter-aware tools.
    /// Default off. Bare `--include-external` = true; explicit form
    /// `--include-external=true|false`.
    #[arg(
        long,
        global = true,
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
        require_equals = true
    )]
    include_external: bool,

    #[command(subcommand)]
    command: Command,
}

// NOTE: `#[expect(clippy::large_enum_variant)]` used to sit here. It became
// UNFULFILLED once `DockerCache`'s Clean variant grew its toolchain flags —
// the size disparity that triggered the lint is gone. `expect` exists to
// surface exactly that, so the suppression is removed rather than downgraded to
// an `allow` that would quietly rot. If the disparity returns, so does the lint.
#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize `.kenn/` and a starter `kenn.toml`.
    Init {
        /// Re-run detection against an existing config, merging the results in
        /// (never overwriting a key you set). Also required to replace a
        /// `kenn.toml` that fails to parse.
        #[arg(long)]
        force: bool,
        /// For a language whose local toolchain is missing, author
        /// `runtime = "docker"` + a published image instead of degrading to
        /// text — needs a runnable docker daemon (see `docker-indexer-runtime`).
        #[arg(long)]
        docker: bool,
    },
    /// Run an indexer pass; flips `live` to the new run on success.
    Index {
        /// Bypass the git-aware staleness skip.
        #[arg(long)]
        force: bool,
        /// Emit one JSON line per progress event instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Repack the vector sidecars as canonical CI packs (D13). Newly
        /// embedded vectors are written as `pack-{hash}.bin` instead of
        /// `seg-{hash}.bin`; at end of run, any pre-existing `seg-*.bin`
        /// is promoted to `pack-*.bin` via rename (content-preserving).
        /// The flag is intended for CI; running locally produces
        /// commit-eligible files in `.kenn/vectors/`.
        #[arg(long)]
        repack: bool,
    },
    /// Print current snapshot info, key counts, and any pending warnings.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Atomically flip `live` to the previous retained snapshot.
    Rollback {
        /// Skip the interactive confirmation. Required in non-TTY contexts.
        #[arg(long)]
        yes: bool,
    },
    /// Speak MCP over stdio against this workspace's snapshot.
    Mcp,
    /// Embed the symbols `kenn index` left null and append a sidecar
    /// segment — the incremental embedding pass. Run after `kenn index`;
    /// the MCP server runs this automatically on cold start.
    Embed,
    /// Re-embed the whole committed code index — the model-swap pass.
    /// Use after changing the embedding model; the everyday path is
    /// `kenn embed`.
    Update,
    /// Evict least-recently-used vector-cache generations past the
    /// `[vectors] cache_cap_mb` size cap (never the active generation,
    /// never committed `pack-*.bin` dirs). Also runs lazily before every
    /// embed pass.
    Gc,
    /// Probe the embedder and report its health: dimension, latency, and the
    /// active backend, or the exact backend error. Diagnoses a silently
    /// degraded embedder (e.g. the macOS fork+Metal bug) that search would
    /// otherwise fall back around to lexical-only.
    Doctor,
    /// Render the graph (`[index] graph_path`, default `kenn_graph.html`)
    /// from the live snapshot's aggregated graph (and persisted analysis,
    /// once available).
    Visualize {
        /// Anchor layout algorithm: `spectral` (default), `force`,
        /// `stress`, or `linlog`. When omitted, the value comes from
        /// `[visualize] layout` in `kenn.toml` if set, otherwise
        /// `spectral`.
        #[arg(long)]
        algo: Option<String>,
    },
    /// Manage the per-user kenn server daemon — embeddings and (in
    /// future) inter-agent / hook-memory capabilities. See
    /// `docs/server.md` for the multi-user-host caveat.
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },
    /// Print a shell completion script to stdout. Supported shells:
    /// `bash`, `zsh`, `fish`, `powershell`, `elvish`. Example:
    /// `kenn completions fish > ~/.config/fish/completions/kenn.fish`.
    Completions { shell: Shell },
    /// Capture a Claude Code hook event into the global kenn collector
    /// store. Each invocation reads one hook JSON payload on stdin and
    /// writes directly to `<state_dir>/collector.db`. Always exits 0 —
    /// failures land in `<state_dir>/cc-hook.log` so capture never
    /// interrupts the user's session. See `kenn cc-hook install` for the
    /// hook-config snippet.
    CcHook {
        #[command(subcommand)]
        action: cmd_cc_hook::CcHookAction,
    },
    /// List and remove kenn's Docker cache volumes (the per-repo dependency and
    /// per-worktree build volumes the docker runtime creates). Config-free —
    /// operates on volume labels and the current worktree root.
    DockerCache {
        #[command(subcommand)]
        action: cmd_docker_cache::DockerCacheAction,
    },
    /// The query + knowledge surface (`overview`, `find`, `list`, `check`,
    /// `findings`, `get`) — flattened in, so these read as top-level commands.
    #[command(flatten)]
    Query(cmd_query::QueryCommand),
}

#[derive(Debug, Subcommand)]
enum ServerAction {
    /// Start the per-user kenn server. Daemonizes by default; runs in
    /// the foreground with `--foreground` (suitable for systemd /
    /// launchd). `--idle-timeout N` enables process-idle exit after
    /// N seconds of no requests — the auto-spawn helper passes 600;
    /// human invocations normally don't.
    Start {
        #[arg(long)]
        foreground: bool,
        #[arg(long, value_name = "SECS")]
        idle_timeout: Option<u64>,
    },
    /// Send SIGTERM (then SIGKILL after grace) to the running daemon
    /// via the PID file. Stale PID files are cleaned up.
    Stop,
    /// Report whether the daemon is running, plus a `/healthz` probe.
    Status,
}

fn main() -> ExitCode {
    let result = try_main();
    // Free the embedding model before the process tears down — the
    // bundled llama.cpp asserts at Metal-device teardown otherwise.
    kenn_store::release_shared_embedder();
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCodes::Generic.into()
        }
    }
}

/// Default tracing filter when `RUST_LOG` is unset: keep kenn at `info`.
const DEFAULT_LOG_FILTER: &str = "info";

fn try_main() -> Result<ExitCode> {
    // Route all `tracing::{info,warn,debug,error}!` calls from kenn
    // through a stderr formatter. Stdout is reserved for MCP JSON-RPC,
    // so stderr is the standard sink. `try_init` is idempotent and
    // returns Err only when another subscriber is already installed
    // (impossible in our single-binary flow); ignore that.
    drop(
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            // Emit a log line when each span closes, carrying the span's
            // open→close duration. This is what surfaces per-tool-call timing
            // from the `mcp.tool` span opened in `kenn_mcp::server::call_tool`.
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
            )
            .try_init(),
    );

    let cli = Cli::parse();

    // `completions` is context-free — it walks the clap Command tree
    // and prints. Skip workspace discovery, config load, and embedder
    // opt-in so the command works outside any kenn workspace.
    if let Command::Completions { shell } = &cli.command {
        return Ok(cmd_completions::run(*shell).into());
    }

    // `cc-hook` sits in the user's Claude Code interactive loop and
    // must be cheap + silent. Skip workspace discovery, config load,
    // and embedder opt-in entirely; cc-hook does its own minimal
    // workspace resolution and silently exits 0 when there is no
    // initialized kenn workspace to capture into.
    if let Command::CcHook { action } = cli.command {
        return Ok(cmd_cc_hook::run_standalone(action).into());
    }

    // `docker-cache` manages kenn's Docker cache volumes purely by their labels
    // and the current worktree root — no kenn.toml, no store layout, no embedder.
    // Dispatch it standalone like `completions`/`cc-hook`.
    if let Command::DockerCache { action } = cli.command {
        return Ok(cmd_docker_cache::run(action)?.into());
    }

    // Opt this process into embedding — index runs and the MCP server
    // produce / query vectors. The model itself stays unloaded until
    // the first embed call (lazy), so commands that never embed pay
    // nothing. Loads the global config once here and threads the model
    // id through the commands that stamp the sidecar manifest, plus
    // hands the full config to the shared embedder for backend
    // selection. No other site re-loads it from disk.
    let global_cfg = kenn_config::GlobalConfig::load().unwrap_or_default();
    let model_id = global_cfg.embeddings.model.clone();
    kenn_store::init_shared_embedder(global_cfg);
    let (workspace, ws_source, ws_reason) =
        resolve_workspace(cli.workspace.as_deref()).context("no workspace identified")?;
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(|| workspace.join("kenn.toml"));

    // `init` runs BEFORE the config load below, which errors on a malformed
    // `kenn.toml` — a cloned repo carrying an incompatible config would
    // otherwise brick every command, `init` included, before it could repair
    // it. `init` does its own tolerant load and layout resolution.
    if let Command::Init { force, docker } = cli.command {
        return Ok(cmd_init::run(&workspace, &config_path, force, docker)?.into());
    }

    // Load config and resolve the store layout once — every subcommand
    // operates against the same resolved `Layout`.
    let config = kenn_config::Config::load_or_default(&config_path)
        .with_context(|| format!("load {}", config_path.display()))?;
    // Absolutize the workspace so the resolved Layout (run dirs, scip output
    // paths) is absolute — required by the docker runtime's same-path mount
    // (`docker run -v` needs absolute paths), and harmless for local runs.
    // Canonicalizes like `Workspace::new`, so the mount path and the store paths
    // share one root. `init` short-circuited above, so the path exists here.
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolve workspace {}", workspace.display()))?;
    let layout =
        kenn_store::Layout::resolve(&config, &workspace).context("resolve store layout")?;

    // Universal query facets — captured before the command is moved out of
    // `cli`. Threaded into every query command via `query_ctx`.
    let (include_tests, include_external) = (cli.include_tests, cli.include_external);

    let ctx = CommandCtx {
        layout,
        config,
        model_id,
        workspace,
        ws_source,
        ws_reason,
        include_tests,
        include_external,
    };
    Ok(dispatch_command(cli.command, ctx)?.into())
}

/// Everything the workspace-bound subcommands consume, resolved once by
/// [`try_main`] before dispatch.
struct CommandCtx {
    layout: kenn_store::Layout,
    config: kenn_config::Config,
    model_id: String,
    workspace: PathBuf,
    ws_source: WorkspaceSource,
    ws_reason: Option<&'static str>,
    include_tests: bool,
    include_external: bool,
}

/// The one-arm-per-subcommand dispatch table — split out of [`try_main`]
/// so the setup logic and the branch-heavy (but trivial) mapping are
/// measured separately.
fn dispatch_command(command: Command, ctx: CommandCtx) -> Result<ExitCodes> {
    let CommandCtx {
        layout,
        config,
        model_id,
        workspace,
        ws_source,
        ws_reason,
        include_tests,
        include_external,
    } = ctx;
    let code = match command {
        // `Init` is handled in `try_main` before the config load — see there.
        #[expect(
            clippy::unreachable,
            reason = "init and docker-cache are dispatched in try_main before the config load; this arm exists only for match exhaustiveness"
        )]
        Command::Init { .. } | Command::DockerCache { .. } => {
            unreachable!("init/docker-cache are dispatched before the config load")
        }
        Command::Index {
            force,
            json,
            repack,
        } => cmd_index::run(layout, config, force, json, repack)?,
        Command::Status { json } => cmd_status::run(layout, json)?,
        Command::Rollback { yes } => cmd_rollback::run(layout, yes)?,
        Command::Mcp => {
            // Workspace-resolution provenance — same target as the
            // post-handshake rebind log emitter in
            // `indexing::rebind_workspace`, so a user grepping for
            // the discovery target sees the full lifecycle (initial
            // bind + any rebinds) in one stream. `%` prefix on
            // fields requests Display, giving unquoted output.
            tracing::info!(
                target: WORKSPACE_DISCOVERY_TARGET,
                source = %ws_source.as_str(),
                path = %workspace.display(),
                reason = %ws_reason.unwrap_or(""),
                "workspace bound"
            );
            cmd_mcp::run(layout, config, ws_source, model_id.clone())?
        }
        Command::Embed => cmd_embed::run(layout, config, &model_id)?,
        Command::Update => cmd_update::run(layout, config)?,
        Command::Gc => cmd_gc::run(&layout, &model_id)?,
        Command::Doctor => cmd_doctor::run(&model_id)?,
        Command::Visualize { algo } => cmd_visualize::run(layout, config, algo.as_deref())?,
        Command::Server { action } => cmd_server::run(action)?,
        Command::Query(q) => cmd_query::dispatch(
            q,
            query_ctx(
                layout,
                config,
                model_id,
                ws_source,
                include_tests,
                include_external,
            ),
        )?,
        // Short-circuited in `try_main` so we don't pay for workspace
        // discovery / config load / embedder opt-in for commands
        // that don't need them: `completions` walks the clap tree;
        // `cc-hook` runs in the user's Claude Code interactive loop
        // and must be cheap + silent.
        #[expect(clippy::unreachable, reason = "handled before workspace resolution")]
        Command::Completions { .. } | Command::CcHook { .. } => unreachable!(),
    };
    Ok(code)
}

use kenn_mcp::{WorkspaceSource, WORKSPACE_DISCOVERY_TARGET};

/// Bundle the resolved workspace context threaded into every query command.
fn query_ctx(
    layout: kenn_store::Layout,
    config: kenn_config::Config,
    model_id: String,
    source: WorkspaceSource,
    include_tests: bool,
    include_external: bool,
) -> cmd_query::Ctx {
    cmd_query::Ctx {
        layout,
        config,
        model_id,
        source,
        include_tests,
        include_external,
    }
}

fn resolve_workspace(
    explicit: Option<&std::path::Path>,
) -> Result<(PathBuf, WorkspaceSource, Option<&'static str>)> {
    if let Some(p) = explicit {
        if !p.exists() {
            anyhow::bail!("workspace path does not exist: {}", p.display());
        }
        return Ok((p.to_path_buf(), WorkspaceSource::CliFlag, None));
    }
    // CLAUDE_PROJECT_DIR: Claude Code's pre-handshake spawn signal.
    // Set on every MCP subprocess by Claude Code; confirmed via the
    // `debug_env` MCP tool. Other hosts (Cursor, Zed) don't set this;
    // they fall through to git-toplevel / cwd and rely on the
    // post-handshake `roots/list` rebind (separate change).
    //
    // `fallthrough_reason` records *why* we couldn't bind from the
    // env var, so the caller's startup log can attribute the
    // fall-through. Pre-handshake values only; post-handshake
    // refinement (roots-capability / roots-empty / non-file) lands
    // with §4-§7 of the roots-discovery change.
    let fallthrough_reason: Option<&'static str> = match std::env::var("CLAUDE_PROJECT_DIR") {
        Ok(raw) => {
            let p = PathBuf::from(&raw);
            if p.is_dir() {
                return Ok((p, WorkspaceSource::ClaudeProjectDir, None));
            }
            eprintln!(
                "kenn: CLAUDE_PROJECT_DIR={raw:?} is not an existing directory; \
                 falling through to git-toplevel / cwd"
            );
            Some("claude-project-dir-invalid")
        }
        Err(_) => Some("no-claude-project-dir"),
    };
    if let Some(p) = git_toplevel() {
        return Ok((p, WorkspaceSource::GitToplevel, fallthrough_reason));
    }
    Ok((
        std::env::current_dir()?,
        WorkspaceSource::Cwd,
        fallthrough_reason,
    ))
}

fn git_toplevel() -> Option<PathBuf> {
    kenn_store::git::work_dir(&std::env::current_dir().ok()?)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// Validate the whole clap command tree (arg-name uniqueness across every
    /// subcommand, etc.) in one call — catches flag collisions such as a group
    /// that both flattens the filter block and declares its own `--kind`.
    #[test]
    fn command_tree_is_valid() {
        super::Cli::command().debug_assert();
    }
}
