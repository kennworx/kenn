//! Thin binary wrapper around the `kenn-server` library.
//!
//! In v1, this binary exists primarily as the target for fork-exec
//! from the auto-spawn helper. The user-facing entrypoint is
//! `kenn server start`, which lives in the main `kenn` CLI (§6) and
//! dispatches into `kenn_server::host::Host::serve` directly without
//! shelling out to this binary.
//!
//! Arguments are intentionally minimal — `--foreground` (default
//! foreground here; the `kenn server start` subcommand handles
//! daemonization before dispatch) and `--idle-timeout <secs>`.

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use kenn_server::{host::HostConfig, paths, Host, Result};

fn main() {
    if let Err(e) = run() {
        eprintln!("kenn-server: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    init_tracing();
    let host_cfg = resolve_host_config()?;
    serve(host_cfg)
}

/// Default tracing filter when `RUST_LOG` is unset: keep kenn at `info`.
const DEFAULT_LOG_FILTER: &str = "info";

/// Best-effort stderr tracing init. Tests / re-entry may have already
/// installed a subscriber; the second call returns Err and we just
/// log a notice.
fn init_tracing() {
    if let Err(e) = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .with_writer(std::io::stderr)
        .try_init()
    {
        eprintln!("kenn-server: tracing init skipped: {e}");
    }
}

/// Read CLI args + global config and assemble the `HostConfig`.
fn resolve_host_config() -> Result<HostConfig> {
    let args: Vec<String> = std::env::args().collect();
    let idle_timeout = parse_idle_timeout(&args)?;
    let cfg = kenn_config::GlobalConfig::load()?;
    let addr = SocketAddr::from_str(&cfg.server.addr).map_err(|e| {
        kenn_server::ServerError::Other(format!("invalid server addr `{}`: {e}", cfg.server.addr))
    })?;
    Ok(HostConfig {
        addr,
        pid_path: paths::pid_file()?,
        idle_timeout,
    })
}

/// Build the runtime and serve.
fn serve(host_cfg: HostConfig) -> Result<()> {
    let host = Host::new(host_cfg);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(host.serve())
}

/// Tiny hand-rolled flag parser for `--idle-timeout <secs>`. The main
/// `kenn` CLI uses clap; this binary is the fork-exec target and
/// keeps deps minimal.
fn parse_idle_timeout(args: &[String]) -> Result<Option<Duration>> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--idle-timeout" {
            let v = iter.next().ok_or_else(|| {
                kenn_server::ServerError::Other("--idle-timeout needs a value".into())
            })?;
            let secs: u64 = v.parse().map_err(|e| {
                kenn_server::ServerError::Other(format!("--idle-timeout `{v}`: {e}"))
            })?;
            return Ok(Some(Duration::from_secs(secs)));
        }
    }
    Ok(None)
}
