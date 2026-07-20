//! Workspace-roots discovery + rebind (mcp-roots-discovery): URI
//! selection, the pure rebind decision, and the rebind driver.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::service::Peer;
use rmcp::RoleServer;

use crate::tools::ServerState;

use super::{emit_code_updated, set_failed, spawn_recovery_pipeline};

// ── workspace rebind (mcp-roots-discovery §5/§6) ─────────────────
//
// `resolve_roots_and_maybe_rebind` is invoked from
// `ServerHandler::on_initialized` (initial post-handshake check) and
// `ServerHandler::on_roots_list_changed` (subsequent host-side
// workspace changes). It queries `roots/list`, picks the first
// `file://` root, and either no-ops (workspace already matches) or
// kicks off `rebind_workspace`.

/// Pure URI-selection: pick the first `file:///`-shape URI, return
/// the ignored ones in input order. Lifted out so the matrix of
/// "first is file", "first is non-file then file", "all non-file",
/// and "multi-file" cases is independently testable.
///
/// Only the local-path form `file:///path` (triple slash, empty
/// authority) is accepted. `file://host/path` (with an authority
/// component) is rejected — kenn binds to local filesystems only,
/// and an authority-bearing URI strongly suggests a remote-FS host
/// we wouldn't know how to index. All MCP hosts in the field
/// (Claude Code, Cursor, Zed) emit the triple-slash form.
#[must_use]
pub fn pick_first_file_root(uris: &[String]) -> (Option<std::path::PathBuf>, Vec<String>) {
    let mut chosen: Option<std::path::PathBuf> = None;
    let mut ignored: Vec<String> = Vec::new();
    for uri in uris {
        // `file:///` matches the triple-slash form exactly; the
        // remainder is the absolute path starting at `/`.
        let local_path = uri.strip_prefix("file://").and_then(|rest| {
            if rest.starts_with('/') {
                Some(std::path::PathBuf::from(rest))
            } else {
                // `file://authority/...` — reject; we don't index
                // remote / non-local filesystems.
                tracing::warn!("kenn-mcp: rejecting non-local file:// URI with authority: {uri}");
                None
            }
        });
        match local_path {
            Some(p) if chosen.is_none() => chosen = Some(p),
            Some(_) => ignored.push(uri.clone()),
            None if chosen.is_some() => ignored.push(uri.clone()),
            None => {} // non-file or rejected file:// before any choice — silently skip
        }
    }
    (chosen, ignored)
}

/// What `decide_roots_resolution` says should happen with a
/// `roots/list` response, given the currently-bound source root.
/// Pure value; dispatch lives in `resolve_roots_and_maybe_rebind`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RootsResolution {
    /// The response had no usable `file://` URI. Caller logs the
    /// reason and leaves the binding as-is.
    NoUsableRoot,
    /// The chosen root matches the currently-bound source root.
    /// Caller promotes `workspace_source` to `RootsList` (the host
    /// has confirmed the tentative bind) but does not rebind layout.
    ConfirmTentative(PathBuf),
    /// The chosen root differs. Caller rebinds to it.
    Rebind(PathBuf),
}

/// Pure decision: given the URIs the client returned and the
/// currently-bound source root, what should happen? Also returns the
/// list of URIs ignored *after* a root was chosen — both valid
/// `file://` URIs that lost to `roots[0]` and any non-file URIs that
/// trailed the chosen root. Non-file URIs that appear *before* any
/// choice are silently skipped (schema-invalid, not "extra roots we
/// left out") — they never reach the ignored list. Caller logs the
/// ignored list verbatim.
pub(crate) fn decide_roots_resolution(
    uris: &[String],
    current_source_root: &Path,
) -> (RootsResolution, Vec<String>) {
    let (chosen, ignored) = pick_first_file_root(uris);
    let Some(new_ws) = chosen else {
        return (RootsResolution::NoUsableRoot, ignored);
    };
    if current_source_root == new_ws {
        return (RootsResolution::ConfirmTentative(new_ws), ignored);
    }
    (RootsResolution::Rebind(new_ws), ignored)
}

/// Query the connected client for its `roots/list` and, if the
/// resolved first root differs from the bound workspace, rebind.
/// No-op when the current `workspace_source` is permanent (only
/// `CliFlag`) — the operator's explicit choice wins.
pub async fn resolve_roots_and_maybe_rebind(state: Arc<ServerState>, peer: Peer<RoleServer>) {
    if state.workspace_source().is_permanent() {
        return;
    }
    // Serialize against any concurrent rebind dispatch — `on_initialized`
    // and `on_roots_list_changed` both land here. Held across the
    // `list_roots().await` so a late notification doesn't sneak past
    // the workspace check. No re-check needed inside the lock:
    // rebind only ever promotes to `RootsList` (tentative), never
    // `CliFlag` (permanent), so the outer guard remains valid.
    let rebind_guard = state.rebind_lock.lock().await;
    let roots = match peer.list_roots().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("kenn-mcp: roots/list call failed: {e}");
            return;
        }
    };
    let uris: Vec<String> = roots.roots.iter().map(|r| r.uri.clone()).collect();
    let current = state.layout().source_root().to_path_buf();
    let (decision, ignored) = decide_roots_resolution(&uris, &current);
    if !ignored.is_empty() {
        tracing::info!("kenn-mcp: ignored roots beyond the first: {ignored:?}");
    }
    match decision {
        RootsResolution::NoUsableRoot => {
            tracing::info!("kenn-mcp: roots/list returned no usable file:// root");
        }
        RootsResolution::ConfirmTentative(_) => {
            state.set_workspace_source(crate::state::WorkspaceSource::RootsList);
            tracing::info!(
                target: crate::state::WORKSPACE_DISCOVERY_TARGET,
                source = %"roots-list",
                path = %current.display(),
                "host confirms tentative bind"
            );
        }
        RootsResolution::Rebind(new_ws) => {
            tracing::info!(
                target: crate::state::WORKSPACE_DISCOVERY_TARGET,
                from = %current.display(),
                to = %new_ws.display(),
                "rebind triggered by roots/list"
            );
            // Clone the Arc for the rebind call so the rebind_lock guard
            // (still held below) can drop after rebind_workspace returns,
            // satisfying the borrow checker while preserving exclusivity.
            rebind_workspace(
                Arc::clone(&state),
                new_ws,
                crate::state::WorkspaceSource::RootsList,
            )
            .await;
        }
    }
    drop(rebind_guard);
}

/// Atomically rebind to a new workspace. Flips the lifecycle to
/// `Failed` (so in-flight tool calls drain to `INDEX_UNAVAILABLE`),
/// swaps the layout, updates the workspace-source tag, and kicks off
/// the recovery pipeline against the new workspace. The poll task
/// already reads `state.layout()` on every tick, so it picks up the
/// new layout automatically.
///
/// Caveat: any in-flight indexing pipeline that was running against
/// the OLD layout continues to completion (no clean abort hook yet);
/// its result publication is harmless because the lifecycle is
/// `Failed` by the time it tries to install. The recovery pipeline
/// then takes the new layout to `Ready`.
pub async fn rebind_workspace(
    state: Arc<ServerState>,
    new_workspace: std::path::PathBuf,
    source: crate::state::WorkspaceSource,
) {
    set_failed(
        &state,
        format!("rebinding workspace to {}", new_workspace.display()),
    );

    let new_layout = match kenn_store::Layout::resolve(&state.config, &new_workspace) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                "kenn-mcp: rebind aborted — Layout::resolve({}) failed: {e}",
                new_workspace.display()
            );
            return;
        }
    };
    state.set_layout(new_layout);
    state.set_workspace_source(source);

    spawn_recovery_pipeline(Arc::clone(&state));

    if let Some(peer) = state.peer.get() {
        // No snapshot timestamp yet — recovery hasn't run. Use a
        // sentinel that names the cause; the agent reads this as a
        // signal to refetch state, not as a precise wall-clock.
        emit_code_updated(peer, "workspace-rebind").await;
    }

    tracing::info!(
        target: crate::state::WORKSPACE_DISCOVERY_TARGET,
        source = %source.as_str(),
        path = %new_workspace.display(),
        "workspace rebound"
    );
}

/// Re-read `kenn.toml` from disk at the workspace root and return the
/// resulting [`kenn_config::Config`]. Falls back to `fallback.clone()`
/// when the file is missing or fails to parse — the prior cached config
/// is always a safe default.
///
/// Called at every reindex trigger so toml edits between reindexes
/// land without requiring an MCP restart (e.g. tweaking
/// `[language.rust] max_threads` or `low_priority` to throttle the
/// indexer's CPU usage).
pub(crate) fn reload_kenn_toml(
    layout: &kenn_store::Layout,
    fallback: &kenn_config::Config,
) -> kenn_config::Config {
    let path = layout.source_root().join("kenn.toml");
    match kenn_config::Config::load_or_default(&path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                target: "kenn_mcp::indexing",
                error = %e,
                path = %path.display(),
                "failed to re-read kenn.toml; using cached config"
            );
            fallback.clone()
        }
    }
}
