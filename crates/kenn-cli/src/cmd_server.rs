//! `kenn server` subcommand — dispatches to the kenn-server library.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::exit::ExitCodes;
use crate::ServerAction;

#[expect(
    clippy::needless_pass_by_value,
    reason = "consumed via the match — clippy can't see the move through enum-variant destructure"
)]
pub fn run(action: ServerAction) -> Result<ExitCodes> {
    match action {
        ServerAction::Start {
            foreground,
            idle_timeout,
        } => start(foreground, idle_timeout),
        ServerAction::Stop => stop(),
        ServerAction::Status => status(),
    }
}

/// Drive a single async block to completion on a one-shot current-thread
/// runtime. Used by `stop` / `status` / the polling loops in `start` and
/// `wait_for_drain` — they need to await `kenn_embed::spawn`'s async
/// helpers from a sync subcommand entry point, but the heavyweight
/// multi-thread runtime that `serve_until_shutdown` builds is reserved
/// for the long-running server itself.
fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime")
        .block_on(f)
}

/// Set by the parent `kenn server start` (daemon mode) on the spawned
/// child. Tells the child "you're the one who should daemonize and
/// serve; the parent is polling /healthz and will return once you're
/// listening." Not a public knob — exists only as the
/// parent↔child handoff signal.
const DAEMONIZE_HANDOFF_ENV: &str = "__KENN_DAEMONIZE_FROM_PARENT";

/// Three control-flow paths through `kenn server start`. Extracted
/// from [`start`] so the branching is unit-testable without invoking
/// the blocking side-effects that each mode performs.
#[derive(Debug, PartialEq, Eq)]
enum StartMode {
    /// `--foreground`, no parent handoff: build the host directly and
    /// stay attached to the invoking shell.
    ForegroundDirect,
    /// Parent kenn server daemon spawned us; daemonize then serve. The
    /// parent is polling /healthz and will return once we're listening.
    ForegroundFromHandoff,
    /// User typed `kenn server start` without `--foreground`: spawn
    /// ourselves with the handoff env var and poll /healthz.
    SpawnDaemon,
}

/// Pure dispatcher for [`start`]. `foreground` is the user's
/// `--foreground` flag; `from_handoff` is whether the
/// `DAEMONIZE_HANDOFF_ENV` env var is set on this process.
fn decide_start_mode(foreground: bool, from_handoff: bool) -> StartMode {
    match (foreground, from_handoff) {
        (_, true) => StartMode::ForegroundFromHandoff,
        (true, false) => StartMode::ForegroundDirect,
        (false, false) => StartMode::SpawnDaemon,
    }
}

fn start(foreground: bool, idle_timeout: Option<u64>) -> Result<ExitCodes> {
    let from_handoff = std::env::var_os(DAEMONIZE_HANDOFF_ENV).is_some();
    match decide_start_mode(foreground, from_handoff) {
        StartMode::ForegroundFromHandoff => run_foreground(idle_timeout, true),
        StartMode::ForegroundDirect => run_foreground(idle_timeout, false),
        StartMode::SpawnDaemon => spawn_daemon_and_wait(idle_timeout),
    }
}

/// Foreground entrypoint: resolve config, optionally daemonize, serve
/// until shutdown. Split out of [`start`] so the dispatcher's CC stays
/// low (one call per arm).
fn run_foreground(idle_timeout: Option<u64>, daemonize: bool) -> Result<ExitCodes> {
    let (host_cfg, model_id) = resolve_host_config(idle_timeout)?;
    if daemonize {
        kenn_server::runtime::daemonize().context("daemonize")?;
    }
    serve_until_shutdown(host_cfg, model_id)?;
    Ok(ExitCodes::Ok)
}

/// Spawn `kenn server start --foreground [--idle-timeout N]` as a
/// detached child (with the handoff env var set so it daemonizes
/// itself), poll `/healthz` until the listener answers or a budget
/// elapses, and print the resulting state. Returns when the daemon is
/// confirmed-listening (or we've timed out).
fn spawn_daemon_and_wait(idle_timeout: Option<u64>) -> Result<ExitCodes> {
    use std::time::Duration;

    let cfg = kenn_config::GlobalConfig::load().unwrap_or_default();
    let url = format!("http://{}", cfg.server.addr);
    let healthz = format!("{url}/healthz");

    spawn_foreground_child(idle_timeout)?;

    // Poll readiness. The daemon's startup is fast (bind + module
    // wire) — model load is lazy. A 10 s budget covers slow disks
    // and CI hosts with comfortable headroom.
    if block_on(poll_until_healthy(&healthz, Duration::from_secs(10))) {
        println!("kenn server: {url} — started");
        return Ok(ExitCodes::Ok);
    }
    let state_dir = kenn_server::paths::state_dir()
        .map_or_else(|| "<state-dir>".into(), |p| p.display().to_string());
    anyhow::bail!(
        "kenn-server failed to report healthy at {url} within 10s — check {state_dir}/server.log"
    )
}

/// Open `<state_dir>/server.log` (append) and return a `(stdout, stderr)` stdio
/// pair for the spawned daemon. Split out so the spawn path stays under the
/// CRAP gate (a straight line of fallible setup steps, all untested).
fn child_log_stdio() -> Result<(std::process::Stdio, std::process::Stdio)> {
    let log_path = kenn_server::paths::log_file().context("resolve server.log path")?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    let log_err = log.try_clone().context("clone server.log handle")?;
    Ok((
        std::process::Stdio::from(log),
        std::process::Stdio::from(log_err),
    ))
}

/// Spawn the detached `kenn server start --foreground` child: handoff env set,
/// stdio pointed at `server.log`. The child detaches with setsid only (no fork
/// — see `kenn_server::runtime::daemonize`), so it inherits these FDs directly.
///
/// INVARIANT: do not put the child in its own process group (no
/// `process_group(0)` / setpgid). `setsid` fails with EPERM if the caller is
/// already a process-group leader; a plain spawn leaves the child a non-leader.
fn spawn_foreground_child(idle_timeout: Option<u64>) -> Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().context("std::env::current_exe()")?;
    let mut cmd = Command::new(&exe);
    cmd.args(["server", "start", "--foreground"]);
    if let Some(n) = idle_timeout {
        cmd.args(["--idle-timeout", &n.to_string()]);
    }
    cmd.env(DAEMONIZE_HANDOFF_ENV, "1");
    let (out, err) = child_log_stdio()?;
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    cmd.spawn().context("spawn detached kenn-server child")?;
    Ok(())
}

/// Poll `probe_url` once every 100 ms until it returns a 2xx or `budget`
/// elapses. Returns `true` on success, `false` on timeout. Extracted so
/// the polling loop is one straight-line function instead of inflating
/// its caller's cyclomatic complexity.
async fn poll_until_healthy(probe_url: &str, budget: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    while tokio::time::Instant::now() < deadline {
        if kenn_embed::spawn::probe_healthz(probe_url).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Load global config, parse the bind address, and assemble the
/// `HostConfig` for the daemon. Split out so `start` is a thin
/// orchestrator (linear control flow → low CRAP).
fn resolve_host_config(idle_timeout: Option<u64>) -> Result<(kenn_server::HostConfig, String)> {
    let cfg = kenn_config::GlobalConfig::load().context("load global config")?;
    let addr: SocketAddr = cfg
        .server_addr()
        .with_context(|| format!("parse [server].addr `{}`", cfg.server.addr))?;
    let pid_path = kenn_server::paths::pid_file().context("resolve PID file path")?;
    let host_cfg = kenn_server::HostConfig {
        addr,
        pid_path,
        idle_timeout: idle_timeout.map(Duration::from_secs),
    };
    Ok((host_cfg, cfg.embeddings.model))
}

/// Build the tokio runtime, instantiate the `EmbeddingsModule` +
/// `Host` inside it (the module spawns a worker via `tokio::spawn`),
/// and serve until graceful shutdown. Daemonization, if any, has
/// already happened — tokio runtimes mustn't cross a fork.
fn serve_until_shutdown(host_cfg: kenn_server::HostConfig, model_id: String) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime
        .block_on(async move {
            let module = kenn_server::EmbeddingsModule::new(model_id);
            let host = kenn_server::Host::new(host_cfg).with_module(module);
            host.serve().await
        })
        .context("server.serve")?;
    Ok(())
}

fn stop() -> Result<ExitCodes> {
    let cfg = kenn_config::GlobalConfig::load().unwrap_or_default();
    let url = format!("http://{}", cfg.server.addr);

    // Primary path: HTTP-graceful via POST /admin/shutdown. Works
    // regardless of who owns the daemon as long as we can reach it.
    // The server marks `status: shutting_down`, rejects new
    // capability requests with 503, drains in-flight requests, then
    // releases module resources (model weights) and exits.
    if block_on(try_graceful_shutdown(&url)) {
        println!("kenn server: {url} — stopped (graceful)");
        return Ok(ExitCodes::Ok);
    }

    // HTTP unreachable — try the PID-file fallback in case a local
    // daemon is hung (process alive, HTTP dead).
    let pid_path = kenn_server::paths::pid_file().context("resolve PID file path")?;
    let stopped = kenn_server::runtime::stop(&pid_path).context("stop")?;
    let tail = if stopped {
        "stopped (via PID file, HTTP was unreachable)"
    } else {
        "not running"
    };
    println!("kenn server: {url} — {tail}");
    Ok(ExitCodes::Ok)
}

/// Issue `POST /admin/shutdown` and wait for the listener to actually
/// close. Returns `true` when the graceful path succeeded, `false`
/// when the daemon was unreachable (caller falls back to PID-file kill).
async fn try_graceful_shutdown(base_url: &str) -> bool {
    if !kenn_embed::spawn::request_admin_shutdown(base_url).await {
        return false;
    }
    wait_for_drain(base_url).await;
    true
}

/// Poll `/healthz` until it stops responding (the server has fully
/// exited) or a 15 s budget elapses. The server transitions:
/// `running` → `shutting_down` (in-flight requests drain) →
/// (TCP listener closed; probe fails). We wait for the listener to
/// actually close so the caller can sequence a follow-up `start`.
async fn wait_for_drain(base_url: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let probe = format!("{base_url}/healthz");
    while tokio::time::Instant::now() < deadline {
        if !kenn_embed::spawn::probe_healthz(&probe).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::warn!(
        target: "kenn_cli::cmd_server",
        url = %base_url,
        "server still responding 15s after /admin/shutdown; consider `kenn server stop` again"
    );
}

fn status() -> Result<ExitCodes> {
    let cfg = kenn_config::GlobalConfig::load().unwrap_or_default();
    let url = format!("http://{}", cfg.server.addr);
    // Always probe /healthz — the server might be running externally
    // (no local PID file) or the local PID file might be stale while a
    // foreign daemon is bound to the address. Truth is at the port.
    let responsive = block_on(kenn_embed::spawn::probe_healthz(&format!("{url}/healthz")));

    let pid_path = kenn_server::paths::pid_file().context("resolve PID file path")?;
    let s = kenn_server::runtime::status(&pid_path).context("status")?;

    println!("{}", render_status(&url, responsive, &s));
    Ok(ExitCodes::Ok)
}

/// Pure render kernel — five branches over `(responsive, pid, cleaned_stale)`.
/// Extracted from [`status`] so the branching logic is table-testable
/// without filesystem or network access. The orchestrator at [`status`]
/// owns dependency resolution; this function only formats.
fn render_status(url: &str, responsive: bool, s: &kenn_server::runtime::Status) -> String {
    match (responsive, s.pid) {
        (true, Some(pid)) => format!("kenn server: {url} — running (pid {pid}, healthy)"),
        (true, None) => format!(
            "kenn server: {url} — running externally (responded to /healthz; no local PID file)"
        ),
        (false, Some(pid)) => {
            format!("kenn server: {url} — pid {pid} alive but /healthz unreachable (unresponsive)")
        }
        (false, None) if s.cleaned_stale => {
            format!("kenn server: {url} — not running (stale PID file cleaned up)")
        }
        (false, None) => format!("kenn server: {url} — not running"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(pid: Option<u32>, cleaned_stale: bool) -> kenn_server::runtime::Status {
        kenn_server::runtime::Status {
            pid_path: std::path::PathBuf::from("/tmp/test.pid"),
            pid,
            running: pid.is_some(),
            cleaned_stale,
        }
    }

    #[test]
    fn render_status_table() {
        let url = "http://127.0.0.1:8000";

        // responsive + local pid → "running healthy"
        assert_eq!(
            render_status(url, true, &st(Some(42), false)),
            "kenn server: http://127.0.0.1:8000 — running (pid 42, healthy)"
        );

        // responsive + no pid → "running externally"
        assert_eq!(
            render_status(url, true, &st(None, false)),
            "kenn server: http://127.0.0.1:8000 — running externally (responded to /healthz; no local PID file)"
        );

        // unresponsive + pid alive → "unresponsive"
        assert_eq!(
            render_status(url, false, &st(Some(42), false)),
            "kenn server: http://127.0.0.1:8000 — pid 42 alive but /healthz unreachable (unresponsive)"
        );

        // unresponsive + no pid + cleaned_stale → "stale PID file cleaned up"
        assert_eq!(
            render_status(url, false, &st(None, true)),
            "kenn server: http://127.0.0.1:8000 — not running (stale PID file cleaned up)"
        );

        // unresponsive + no pid + no stale → "not running"
        assert_eq!(
            render_status(url, false, &st(None, false)),
            "kenn server: http://127.0.0.1:8000 — not running"
        );
    }

    #[test]
    fn decide_start_mode_table() {
        // `from_handoff` wins over `foreground` — the parent is polling
        // /healthz and we must daemonize regardless of the flag.
        assert_eq!(
            decide_start_mode(false, true),
            StartMode::ForegroundFromHandoff
        );
        assert_eq!(
            decide_start_mode(true, true),
            StartMode::ForegroundFromHandoff
        );

        // No handoff: --foreground stays attached.
        assert_eq!(decide_start_mode(true, false), StartMode::ForegroundDirect);

        // No handoff, no --foreground: user typed `kenn server start`,
        // we spawn a detached daemon.
        assert_eq!(decide_start_mode(false, false), StartMode::SpawnDaemon);
    }
}
