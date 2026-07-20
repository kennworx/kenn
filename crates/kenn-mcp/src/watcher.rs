//! In-process file watcher for `kenn mcp`.
//!
//! Pipeline: `notify` events → filter (source extensions + project
//! files; minus `WORKSPACE_SKIP_DIRS` and user `[exclude] globs`) →
//! debounce (`mcp.watch_debounce_ms` of idle) → `spawn_background_reindex`.
//!
//! The watcher does NOT bypass the staleness key or the one-writer
//! flock — it just *triggers* the same reindex path the `reindex`
//! tool uses. See design.md §D1–D5 and the `mcp-orchestrated-indexing`
//! spec.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_model::{Language, ProjectFile};
use kenn_store::staleness::WORKSPACE_SKIP_DIRS;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};

use crate::indexing::spawn_background_reindex;
use crate::state::WatcherState;
use crate::tools::{ServerState, WatchStartResult};

/// Construct a `globset::GlobSet` from a list of user-supplied glob
/// strings. Invalid patterns are skipped with a warning — the watcher
/// must not refuse to start because a single config glob is malformed.
fn build_exclude_globs(globs: &[String]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for g in globs {
        match Glob::new(g) {
            Ok(glob) => {
                b.add(glob);
            }
            Err(e) => {
                tracing::warn!("kenn-mcp/watcher: ignoring invalid exclude glob {g:?}: {e}");
            }
        }
    }
    b.build().unwrap_or_else(|e| {
        tracing::warn!("kenn-mcp/watcher: glob set build failed ({e}); using empty set");
        GlobSet::empty()
    })
}

/// Set of source-file extensions to watch — union of
/// `Language::extensions()` across every variant. Computed once per
/// watcher start.
fn source_extensions() -> Vec<&'static str> {
    let mut v = Vec::new();
    for lang in [
        Language::Csharp,
        Language::TypeScript,
        Language::Rust,
        Language::Go,
        Language::Python,
    ] {
        v.extend_from_slice(lang.extensions());
    }
    v
}

/// All `ProjectFile` matchers — union of `Language::project_files()`
/// across every variant.
fn project_file_matchers() -> Vec<ProjectFile> {
    let mut v = Vec::new();
    for lang in [
        Language::Csharp,
        Language::TypeScript,
        Language::Rust,
        Language::Go,
        Language::Python,
    ] {
        v.extend_from_slice(lang.project_files());
    }
    v
}

/// Absolute, existing external-vault markdown roots — the absolute root globs
/// in [`kenn_config::MarkdownConfig`]. In-repo (relative) roots live under the
/// workspace and are already covered by the recursive workspace watch, so only
/// external vaults need their own notify watch + filter clause. Empty when
/// markdown indexing is disabled.
fn external_md_roots(md: &kenn_config::MarkdownConfig) -> Vec<std::path::PathBuf> {
    if !md.enabled {
        return Vec::new();
    }
    md.roots
        .iter()
        .filter_map(|r| {
            let p = Path::new(&r.glob);
            (p.is_absolute() && p.is_dir())
                .then(|| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
        })
        .collect()
}

/// Whether `path` is an index-affecting markdown event under an external vault
/// root: a `.md`/`.markdown` file beneath one of `vault_roots`, not pruned by
/// the markdown exclude globs (matched vault-relative). The in-repo `.md` case
/// is handled by [`path_passes_filter`] (its extension list includes markdown
/// when enabled), not here.
pub(crate) fn md_event_passes(
    path: &Path,
    vault_roots: &[std::path::PathBuf],
    md_excludes: &GlobSet,
) -> bool {
    let is_md = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let ext = e.to_ascii_lowercase();
        Language::Markdown.extensions().contains(&ext.as_str())
    });
    if !is_md {
        return false;
    }
    vault_roots
        .iter()
        .any(|root| match path.strip_prefix(root) {
            Ok(rel) => !md_excludes.is_match(rel),
            Err(_) => false,
        })
}

/// Filter decision for a single filesystem event path.
///
/// `ws_root` is the absolute workspace root; `path` is the absolute
/// event path. Returns `true` only when the path passes every clause
/// of the filter (see `mcp-orchestrated-indexing` spec, watcher
/// pipeline §2).
pub(crate) fn path_passes_filter(
    ws_root: &Path,
    path: &Path,
    source_exts: &[&str],
    project_files: &[ProjectFile],
    user_excludes: &GlobSet,
) -> bool {
    // Workspace-relative path. Events for paths outside the workspace
    // are dropped — they can arrive if `notify` follows a symlink
    // pointing outside.
    let Ok(rel) = path.strip_prefix(ws_root) else {
        return false;
    };

    // Skip-dirs check first: anywhere along the relative path, if a
    // component matches a `WORKSPACE_SKIP_DIRS` entry, drop the event.
    // Catches `.git/`, `.kenn/`, `target/`, `node_modules/`, etc., even
    // when they're nested.
    for comp in rel.components() {
        if let std::path::Component::Normal(s) = comp {
            if let Some(name) = s.to_str() {
                if WORKSPACE_SKIP_DIRS.contains(&name) {
                    return false;
                }
            }
        }
    }

    // User exclude globs (matched against workspace-relative paths so
    // patterns like `**/generated/**` work as written).
    if user_excludes.is_match(rel) {
        return false;
    }

    // Extension / project-file match.
    let ext_str = path.extension().and_then(|s| s.to_str());
    if ext_str.is_some_and(|ext| source_exts.contains(&ext)) {
        return true;
    }

    let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    project_files.iter().any(|pf| match pf {
        ProjectFile::Extension(e) => ext_str == Some(*e),
        ProjectFile::Filename(n) => filename == *n,
    })
}

/// Handle returned by [`start`]: holds the watcher alive (drop = stop)
/// and the debounce task's cancel signal.
pub struct WatcherHandle {
    /// The notify watcher. Dropping unregisters all watched paths.
    _watcher: RecommendedWatcher,
    /// Sender side of the debounce task's cancel watch. Setting `true`
    /// causes the task to exit on its next iteration.
    cancel: watch::Sender<bool>,
}

impl WatcherHandle {
    /// Consume the handle: signal the debounce task to cancel and drop
    /// the `notify` watcher (unregistering all watches). Named
    /// `shutdown` rather than `stop` to disambiguate from the
    /// free-function [`stop`] which takes a `&ServerState` and lifts
    /// the handle out of the server's slot before calling `shutdown`
    /// on it.
    pub fn shutdown(self) {
        // Best-effort: a SendError only happens if no receivers exist,
        // which means the debounce task already exited; nothing to do.
        _ = self.cancel.send(true);
        // `self._watcher` drops here, unregistering notify.
    }
}

/// Start the watcher: register the `notify` watcher (recursive, rooted
/// at the workspace), and spawn a debounce task that consumes filtered
/// events and triggers `spawn_background_reindex` after
/// `debounce_ms` of inactivity.
///
/// On `notify::RecommendedWatcher` construction failure, returns the
/// error and updates no state.
///
/// Updates `state.watcher_state` as events flow: `Idle` when the
/// debounce deadline is consumed (or no event has landed yet),
/// `Debouncing` while a deadline is pending. Reset to `Off` only when
/// the handle is dropped/stopped.
pub fn start(state: &Arc<ServerState>) -> Result<(WatchStartResult, WatcherHandle), notify::Error> {
    // Canonicalize the workspace root so the filter's `strip_prefix`
    // matches whatever path the OS hands back in events. macOS in
    // particular reports events under `/private/var/...` even when the
    // watch was registered at `/var/...` (a symlink). Without this,
    // every event would be filtered as "outside workspace".
    let ws_root = state
        .source_root()
        .canonicalize()
        .unwrap_or_else(|_| state.source_root());
    let debounce_ms = state.config.mcp.watch_debounce_ms;
    let user_excludes = build_exclude_globs(&state.config.workspace.excludes);
    // Markdown (when enabled) makes in-repo `.md` index-affecting via the
    // extension list; external vaults get their own watches + filter clause.
    let md_cfg = &state.config.language.markdown;
    let mut source_exts = source_extensions();
    if md_cfg.enabled {
        source_exts.extend_from_slice(Language::Markdown.extensions());
    }
    let vault_roots = external_md_roots(md_cfg);
    let md_excludes = build_exclude_globs(&md_cfg.excludes);
    let project_files = project_file_matchers();
    let git_aware_skip = state.config.staleness.git_aware_skip;
    let config_sig = state.config.indexing_signature();
    let state_for_cb = Arc::clone(state);

    // Exact path of the `live` pointer. The recursive watch already
    // receives its events, but the `.kenn/` skip-dir filter drops them.
    // Carve a precise exception for ONLY this entry (D3 feedback guard):
    // `runs/**` writes stay filtered, so a reindex never re-triggers
    // itself. `None` disables live detection (the backstop still covers
    // cross-instance reload). Canonicalized to match the OS-reported
    // event path (macOS `/private/...`).
    let expected_live: Option<std::path::PathBuf> = {
        let lp = state.layout().live_path();
        lp.parent()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.join("live"))
    };

    // Channel: filtered events → debounce task. Unbounded because
    // `notify` can burst hundreds of events per second; backpressure
    // would block its internal thread.
    let (event_tx, event_rx) = mpsc::unbounded_channel::<()>();
    // Channel: `live`-pointer events → hot-reload task (D3).
    let (live_tx, live_rx) = mpsc::unbounded_channel::<()>();

    // `notify`'s callback runs on its internal thread. Filtering
    // happens inline (cheap; no IO), then we forward a unit "ping"
    // through the unbounded channel — the debounce task is the only
    // consumer.
    let ws_root_for_cb = ws_root.clone();
    let vault_roots_cb = vault_roots.clone();
    let watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            let event = match res {
                Ok(ev) => ev,
                Err(e) => {
                    tracing::debug!("kenn-mcp/watcher: notify error: {e}");
                    return;
                }
            };
            // All event kinds (Create/Modify/Remove/Rename) contribute
            // equally if their paths pass the filter — see design D2.
            for path in &event.paths {
                // `live`-pointer exception (D3): route to hot-reload, not
                // the reindex debounce, and do NOT count it as a source
                // event. Checked before the filter (which drops `.kenn/`).
                if expected_live.as_deref() == Some(path.as_path()) {
                    _ = live_tx.send(());
                    break;
                }
                if path_passes_filter(
                    &ws_root_for_cb,
                    path,
                    &source_exts,
                    &project_files,
                    &user_excludes,
                ) || md_event_passes(path, &vault_roots_cb, &md_excludes)
                {
                    // Bump the event counter so `is_stale` flips
                    // immediately (D4), then wake the debounce task.
                    state_for_cb.bump_event_seq();
                    // Best-effort: a SendError only happens if the
                    // debounce task has stopped; nothing to do.
                    _ = event_tx.send(());
                    // One forwarded ping is enough to wake the
                    // debounce task; don't flood it on multi-path
                    // events.
                    break;
                }
            }
        },
        notify::Config::default(),
    )?;

    // Recursive watch rooted at the workspace.
    let mut watcher = watcher;
    watcher.watch(&ws_root, RecursiveMode::Recursive)?;
    // Extra recursive watches for external markdown vaults (outside the
    // workspace). A failure to watch one vault is logged, not fatal — the
    // workspace watch (the common case) must still start.
    for vault in &vault_roots {
        if let Err(e) = watcher.watch(vault, RecursiveMode::Recursive) {
            tracing::warn!(
                "kenn-mcp/watcher: could not watch markdown vault {}: {e}",
                vault.display()
            );
        }
    }

    // Cancel watch — set true to stop the debounce task.
    let (cancel_tx, cancel_rx) = watch::channel(false);

    // Initial state is Idle (running, no pending deadline).
    state.watcher_state.store(WatcherState::Idle);

    spawn_debounce_task(
        Arc::clone(state),
        event_rx,
        cancel_rx,
        Duration::from_millis(debounce_ms),
    );
    spawn_live_reload_task(Arc::clone(state), live_rx, git_aware_skip, config_sig);

    Ok((
        WatchStartResult {
            started: true,
            debounce_ms,
        },
        WatcherHandle {
            _watcher: watcher,
            cancel: cancel_tx,
        },
    ))
}

/// The debounce loop: wait for an event ping, then sleep until a
/// `debounce` window of inactivity passes, then fire one reindex
/// trigger. Resets the deadline every time a new ping arrives.
fn spawn_debounce_task(
    state: Arc<ServerState>,
    mut event_rx: mpsc::UnboundedReceiver<()>,
    mut cancel_rx: watch::Receiver<bool>,
    debounce: Duration,
) {
    tokio::spawn(async move {
        loop {
            // Defensive check: if `stop()` ran before the task's first
            // poll, cancel is already set. Don't store Idle (would
            // race with the Off store in `stop()`).
            if *cancel_rx.borrow() {
                state.watcher_state.store(WatcherState::Off);
                return;
            }
            // Phase 1: idle — wait for the first event ping (or cancel).
            state.watcher_state.store(WatcherState::Idle);
            tokio::select! {
                biased;
                changed = cancel_rx.changed() => {
                    if changed.is_err() || *cancel_rx.borrow() {
                        state.watcher_state.store(WatcherState::Off);
                        return;
                    }
                }
                ping = event_rx.recv() => {
                    if ping.is_none() {
                        state.watcher_state.store(WatcherState::Off);
                        return; // sender (notify watcher) dropped
                    }
                }
            }

            // Phase 2: debouncing — sleep_until(now + debounce),
            // resetting the deadline whenever a new event arrives.
            state.watcher_state.store(WatcherState::Debouncing);
            let mut deadline = tokio::time::Instant::now() + debounce;
            loop {
                tokio::select! {
                    biased;
                    changed = cancel_rx.changed() => {
                        if changed.is_err() || *cancel_rx.borrow() {
                            state.watcher_state.store(WatcherState::Off);
                            return;
                        }
                    }
                    ping = event_rx.recv() => {
                        if ping.is_none() {
                            state.watcher_state.store(WatcherState::Off);
                            return;
                        }
                        // Drain any pending pings without resetting
                        // beyond `now + debounce` — bursts collapse.
                        while event_rx.try_recv().is_ok() {}
                        deadline = tokio::time::Instant::now() + debounce;
                    }
                    () = tokio::time::sleep_until(deadline) => {
                        // Window elapsed. Trigger and go back to Idle.
                        state
                            .watcher_triggers
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        spawn_background_reindex(Arc::clone(&state));
                        break;
                    }
                }
            }
        }
    });
}

/// Consume `live`-pointer pings and drive snapshot hot-reload (D3). One
/// task per watcher; it exits when the `notify` watcher (the ping
/// sender) is dropped on `stop`. Coalesces a burst of pings into a
/// single reload — `handle_live_event` is idempotent (it dedups against
/// the served run), so an extra reload is at worst a no-op.
fn spawn_live_reload_task(
    state: Arc<ServerState>,
    mut live_rx: mpsc::UnboundedReceiver<()>,
    git_aware_skip: bool,
    config_sig: u64,
) {
    tokio::spawn(async move {
        while live_rx.recv().await.is_some() {
            // Drain coalesced pings: a single retarget can surface as
            // several events.
            while live_rx.try_recv().is_ok() {}
            crate::indexing::handle_live_event(&state, git_aware_skip, config_sig).await;
        }
    });
}

/// Stop the watcher if one is held in `state.watcher`. Idempotent:
/// returns true if a watcher was running, false otherwise.
pub fn stop(state: &ServerState) -> bool {
    let mut g = state.watcher.lock().expect("watcher mutex poisoned");
    if let Some(handle) = g.take() {
        handle.shutdown();
        state.watcher_state.store(WatcherState::Off);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        PathBuf::from("/ws")
    }

    fn exts() -> Vec<&'static str> {
        source_extensions()
    }

    fn pfs() -> Vec<ProjectFile> {
        project_file_matchers()
    }

    fn no_excludes() -> GlobSet {
        GlobSet::empty()
    }

    #[test]
    fn source_file_passes() {
        assert!(path_passes_filter(
            &ws(),
            &ws().join("src/Main.cs"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }

    #[test]
    fn project_file_extension_passes() {
        assert!(path_passes_filter(
            &ws(),
            &ws().join("src/MyApp.csproj"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }

    #[test]
    fn project_file_by_filename_passes() {
        assert!(path_passes_filter(
            &ws(),
            &ws().join("Cargo.toml"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }

    #[test]
    fn skip_dir_blocks_event() {
        assert!(!path_passes_filter(
            &ws(),
            &ws().join("target/debug/Main.cs"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
        assert!(!path_passes_filter(
            &ws(),
            &ws().join(".git/HEAD"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }

    #[test]
    fn user_exclude_glob_blocks_event() {
        let mut b = GlobSetBuilder::new();
        b.add(Glob::new("**/generated/**").unwrap());
        let excludes = b.build().unwrap();
        assert!(!path_passes_filter(
            &ws(),
            &ws().join("src/generated/Foo.cs"),
            &exts(),
            &pfs(),
            &excludes,
        ));
    }

    #[test]
    fn unrelated_extension_blocks() {
        assert!(!path_passes_filter(
            &ws(),
            &ws().join("README.md"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }

    #[test]
    fn in_repo_md_passes_when_markdown_enabled() {
        // With markdown indexing on, the watcher's extension list includes
        // `.md`/`.markdown`, so in-repo docs become index-affecting.
        let mut md_exts = exts();
        md_exts.extend_from_slice(Language::Markdown.extensions());
        assert!(path_passes_filter(
            &ws(),
            &ws().join("docs/guide.md"),
            &md_exts,
            &pfs(),
            &no_excludes(),
        ));
        // …but not when markdown is off (the default extension list).
        assert!(!path_passes_filter(
            &ws(),
            &ws().join("docs/guide.md"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }

    #[test]
    fn external_vault_md_event_passes_and_respects_excludes() {
        let vault = PathBuf::from("/vault");
        let roots = vec![vault.clone()];
        let mut b = GlobSetBuilder::new();
        b.add(Glob::new("**/drafts/**").unwrap());
        let md_excludes = b.build().unwrap();

        // a `.md` under the vault passes
        assert!(md_event_passes(
            &vault.join("daily/today.md"),
            &roots,
            &md_excludes
        ));
        // a non-markdown file does not
        assert!(!md_event_passes(
            &vault.join("daily/today.txt"),
            &roots,
            &md_excludes
        ));
        // an excluded path does not
        assert!(!md_event_passes(
            &vault.join("drafts/wip.md"),
            &roots,
            &md_excludes
        ));
        // a path outside every vault root does not
        assert!(!md_event_passes(
            &PathBuf::from("/elsewhere/note.md"),
            &roots,
            &md_excludes
        ));
    }

    #[test]
    fn outside_workspace_blocks() {
        assert!(!path_passes_filter(
            &ws(),
            &PathBuf::from("/other/Main.cs"),
            &exts(),
            &pfs(),
            &no_excludes(),
        ));
    }
}
