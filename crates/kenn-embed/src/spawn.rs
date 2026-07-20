//! Auto-spawn helper — fork-exec a `kenn server start --idle-timeout N`
//! child and wait for it to report `/healthz`.
//!
//! Used by the embedder selector when `embeddings.url` is unset, a probe
//! of the configured `[server].addr` fails, and we want to bring a kenn
//! server up on demand so multiple workspace processes can share it.

use std::time::Duration;

use tokio::process::Command;
use tokio::time::{sleep, Instant};

/// How long the helper polls `/healthz` after fork before giving up
/// and falling back to in-process inference. 5 s comfortably covers
/// daemonization + bind on every platform we target; the model itself
/// loads lazily on the first `/v1/embeddings` request, not at startup.
const READINESS_BUDGET: Duration = Duration::from_secs(5);
/// Interval between `/healthz` polls.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Try to spawn a kenn server child detached from the current process
/// and wait for it to come up at `addr`.
///
/// Returns `Ok(())` once a `/healthz` 200 is observed. Returns
/// `Err(_)` on spawn failure or readiness timeout. A timeout is NOT a
/// destructive failure — the child may still come up later and serve
/// other processes; this process just falls back to in-process for
/// its own lifetime.
///
/// The child's own bind may race with another concurrent spawn; the
/// loser exits cleanly on `EADDRINUSE` and the post-spawn `/healthz`
/// probe tolerates either outcome (whoever wins the bind is reachable
/// at `addr`).
pub async fn try_spawn_local_server(addr: &str, idle_timeout: Duration) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let idle_secs = idle_timeout.as_secs();
    let mut cmd = Command::new(&exe);
    cmd.args(["server", "start", "--idle-timeout", &idle_secs.to_string()]);
    // Best-effort fire-and-detach. The child either daemonizes itself
    // (Unix: via the `daemonize` crate inside `kenn server start`) or
    // stays in the foreground if it crashes; either way the parent
    // exits and we probe.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // tokio's `Child` defaults to `kill_on_drop = false` — dropping the
    // handle simply releases the FD and lets the child continue running
    // (the daemonize path inside `kenn server start` then detaches it).
    let _child = cmd
        .spawn()
        .map_err(|e| format!("spawn `{}`: {e}", exe.display()))?;

    // Poll the probe.
    let deadline = Instant::now() + READINESS_BUDGET;
    let probe_url = format!("http://{addr}/healthz");
    while Instant::now() < deadline {
        if probe_healthz(&probe_url).await {
            return Ok(());
        }
        sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "kenn server did not report healthy at {addr} within {READINESS_BUDGET:?}"
    ))
}

/// Async health probe — returns `true` only on a 200 from `/healthz`.
/// Network errors, non-2xx, anything else → false.
pub async fn probe_healthz(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    matches!(client.get(url).send().await, Ok(r) if r.status().is_success())
}

/// Async `POST /admin/shutdown` — returns `true` on any 2xx response.
/// Used by `kenn server stop` as the HTTP-graceful primary path before
/// falling back to PID-file SIGTERM.
pub async fn request_admin_shutdown(base_url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    let url = format!("{base_url}/admin/shutdown");
    matches!(client.post(&url).send().await, Ok(r) if r.status().is_success())
}

#[cfg(test)]
mod tests {
    use super::probe_healthz;

    #[tokio::test]
    async fn probe_refused_port_returns_false() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        assert!(!probe_healthz(&format!("http://{addr}/healthz")).await);
    }
}
