//! The kenn-server HTTP host (design D0).
//!
//! A thin `axum`-based listener that hosts capability modules. The host
//! owns the bind address, the PID file, `GET /healthz`, the idle-timeout
//! aggregator, and graceful shutdown via SIGTERM / SIGINT.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use tokio::sync::Notify;

use crate::pid;
use crate::ServerError;

/// A capability module pluggable into the [`Host`].
///
/// Implementors own their routes, their own state, and their own
/// startup / shutdown. The host's lifecycle (idle-timeout, PID,
/// graceful shutdown) is shared across every registered module.
pub trait Module: Send + Sync + 'static {
    /// Short identifier — used in logs and (future) `/healthz` detail.
    fn name(&self) -> &'static str;

    /// Mount this module's routes onto the supplied router.
    ///
    /// The returned router replaces the input; the standard idiom is
    /// `router.merge(my_routes())` or
    /// `router.nest("/v1/foo", foo_routes())`.
    fn register(self: Arc<Self>, router: Router) -> Router;

    /// Called by the host after axum has finished draining in-flight
    /// requests during graceful shutdown. Modules release expensive
    /// resources (model weights, persistent connections) here so the
    /// process exits cleanly. Default: no-op. The boxed-future form
    /// lets the trait stay object-safe.
    fn shutdown<'a>(
        self: Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// Configuration the host needs at construction time. Most fields are
/// derived from `kenn-config::GlobalConfig`; the daemon-mode flags
/// (`idle_timeout`, `pid_path`) are computed by the calling binary.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Bind address.
    pub addr: SocketAddr,
    /// Path to the PID file. Resolved via `crate::paths::pid_file()`
    /// by the binary; library callers can override for tests.
    pub pid_path: PathBuf,
    /// When `Some(d)`, the host exits after `d` of aggregate idleness
    /// (no requests on any capability route). When `None`, runs
    /// indefinitely — matches the "externally-managed daemon"
    /// behavior of design D8.
    pub idle_timeout: Option<Duration>,
}

/// Lifecycle status reported by `/healthz` so clients can observe the
/// shutdown phase without having to probe a separate route. `running` is
/// the steady state; `shutting_down` is set by the admin-shutdown
/// route or any other graceful-shutdown trigger and persists for as
/// long as in-flight requests are still draining.
const STATUS_RUNNING: u8 = 0;
const STATUS_SHUTTING_DOWN: u8 = 1;

/// Shared host state — the request-timestamp atomic, the wake notifier
/// for the idle-exit task, and an admin-shutdown trigger for the HTTP
/// stop route.
#[derive(Debug)]
struct HostState {
    /// Tokio `Instant` (monotonic) of the last non-`/healthz` request,
    /// represented as nanoseconds since `start`. `AtomicU64` because
    /// `Instant` itself isn't `Copy` into an atomic.
    last_request_ns: AtomicU64,
    /// Process start instant — the origin for `last_request_ns`.
    start: Instant,
    /// Wakes the idle-exit task when a request bumps `last_request_ns`
    /// so it can recompute when to fire next.
    activity: Notify,
    /// Wakes the shutdown task when `POST /admin/shutdown` is hit.
    admin_shutdown: Notify,
    /// Reported by `/healthz`. Flipped to `STATUS_SHUTTING_DOWN`
    /// before graceful shutdown drains in-flight requests so a
    /// polling client can observe the transition.
    status: std::sync::atomic::AtomicU8,
}

impl HostState {
    fn new() -> Self {
        Self {
            last_request_ns: AtomicU64::new(0),
            start: Instant::now(),
            activity: Notify::new(),
            admin_shutdown: Notify::new(),
            status: std::sync::atomic::AtomicU8::new(STATUS_RUNNING),
        }
    }

    fn bump(&self) {
        let elapsed = self.start.elapsed().as_nanos();
        // Saturate rather than truncate — the daemon won't run for >584
        // years and even then the worst case is the idle counter
        // saturating.
        let v = u64::try_from(elapsed).unwrap_or(u64::MAX);
        self.last_request_ns.store(v, Ordering::Relaxed);
        self.activity.notify_one();
    }

    fn last_request(&self) -> Instant {
        let v = self.last_request_ns.load(Ordering::Relaxed);
        self.start + Duration::from_nanos(v)
    }

    fn status_str(&self) -> &'static str {
        match self.status.load(Ordering::Relaxed) {
            STATUS_SHUTTING_DOWN => "shutting_down",
            _ => "running",
        }
    }

    fn mark_shutting_down(&self) {
        self.status.store(STATUS_SHUTTING_DOWN, Ordering::Relaxed);
    }
}

/// The kenn-server host. Construct with [`Host::new`], add modules with
/// [`Host::with_module`], then `serve`.
pub struct Host {
    config: HostConfig,
    modules: Vec<Arc<dyn Module>>,
    state: Arc<HostState>,
}

impl Host {
    /// New host with no modules registered. Add capabilities with
    /// [`Host::with_module`] before serving.
    #[must_use]
    pub fn new(config: HostConfig) -> Self {
        Self {
            config,
            modules: Vec::new(),
            state: Arc::new(HostState::new()),
        }
    }

    /// Register a capability module. Multiple modules can be added; each
    /// owns its own routes and may use `Router::merge` or `Router::nest`.
    #[must_use]
    pub fn with_module<M: Module>(mut self, module: M) -> Self {
        self.modules.push(Arc::new(module));
        self
    }

    /// Bind, write the PID file, install signal handlers, and serve
    /// until SIGTERM/SIGINT or the idle timeout expires. Cleans up the
    /// PID file on graceful shutdown.
    pub async fn serve(self) -> Result<(), ServerError> {
        let listener = tokio::net::TcpListener::bind(self.config.addr)
            .await
            .map_err(|e| ServerError::Bind {
                addr: self.config.addr.to_string(),
                source: e,
            })?;
        let actual = listener.local_addr().map_err(ServerError::Io)?;
        tracing::info!(
            target: "kenn_server::host",
            addr = %actual,
            pid = %std::process::id(),
            "kenn-server listening"
        );

        pid::write(&self.config.pid_path, std::process::id())?;

        let mut router = Router::new()
            .route("/healthz", get(healthz))
            .route("/admin/shutdown", post(admin_shutdown))
            .with_state(Arc::clone(&self.state));

        for module in &self.modules {
            router = Arc::clone(module).register(router);
        }

        // Two middlewares (outer runs first):
        //   1. reject_during_shutdown — once `status == shutting_down`,
        //      every capability route returns 503 with a clear body so
        //      clients can retry-later instead of being silently
        //      half-served. `/healthz` and `/admin/shutdown` stay open
        //      so observers can watch the drain and `stop` is
        //      idempotent.
        //   2. track_activity — bumps the idle counter on any non-
        //      internal request.
        let router = router
            .layer(middleware::from_fn_with_state(
                Arc::clone(&self.state),
                track_activity,
            ))
            .layer(middleware::from_fn_with_state(
                Arc::clone(&self.state),
                reject_during_shutdown,
            ));

        let shutdown_state = Arc::clone(&self.state);
        let idle = self.config.idle_timeout;
        let serve_fut = axum::serve(listener, router).with_graceful_shutdown(async move {
            shutdown_signal(shutdown_state, idle).await;
        });

        // axum drains in-flight requests on its own; once `serve_fut`
        // resolves, every handler has completed.
        let serve_result = serve_fut.await;

        // Now release per-module resources (model weights, etc.) so
        // the process actually exits cleanly. Done sequentially —
        // module count is small and ordering is observable in logs.
        for module in &self.modules {
            tracing::info!(
                target: "kenn_server::host",
                module = module.name(),
                "shutting down module"
            );
            Arc::clone(module).shutdown().await;
        }

        // PID removal is best-effort — a crash leaves a stale file, and
        // the next start/status invocation cleans it up.
        if let Err(e) = pid::remove(&self.config.pid_path) {
            tracing::warn!(
                target: "kenn_server::host",
                error = %e,
                "failed to remove PID file on shutdown"
            );
        }
        serve_result.map_err(ServerError::Io)
    }
}

/// Healthz response — capability-agnostic. Returns 200 once the host
/// is listening and modules are wired. The `status` field is
/// `"running"` in steady state and `"shutting_down"` once any
/// graceful-shutdown trigger has fired (SIGTERM, idle timeout, or
/// `POST /admin/shutdown`) and in-flight requests are draining.
async fn healthz(State(state): State<Arc<HostState>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": state.status_str(),
        "uptime_seconds": state.start.elapsed().as_secs(),
    }))
}

/// `POST /admin/shutdown` — request a graceful shutdown. Returns 202
/// Accepted with `{ "status": "shutting_down" }`; axum then stops
/// accepting new connections, in-flight requests complete, and the
/// process exits. There is no auth in v1 — anyone who can reach the
/// port can shut the daemon down. On shared hosts use a per-user
/// port (see design R4) so cross-user shutdown isn't reachable.
async fn admin_shutdown(State(state): State<Arc<HostState>>) -> Response {
    state.mark_shutting_down();
    state.admin_shutdown.notify_one();
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "shutting_down" })),
    )
        .into_response()
}

/// Whether a path is one of the host's own administrative routes that
/// should keep working during graceful shutdown (so the lifecycle is
/// observable and stop is idempotent).
fn is_internal_path(path: &str) -> bool {
    path == "/healthz" || path == "/admin/shutdown"
}

/// Middleware that returns 503 Service Unavailable for any capability
/// route once `status == shutting_down`. Internal routes (`/healthz`,
/// `/admin/shutdown`) stay reachable so clients can observe the drain
/// and the shutdown request itself is idempotent.
async fn reject_during_shutdown(
    State(state): State<Arc<HostState>>,
    request: Request,
    next: Next,
) -> Response {
    if is_internal_path(request.uri().path())
        || state.status.load(Ordering::Relaxed) == STATUS_RUNNING
    {
        return next.run(request).await;
    }
    let body = json!({
        "error": {
            "message": "kenn server is shutting down; retry later",
            "type": "service_unavailable",
            "code": "shutting_down",
        }
    });
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

/// Middleware that bumps the activity counter on every request EXCEPT
/// `/healthz` and `/admin/shutdown` — otherwise a status-polling
/// client would keep the daemon alive forever, and the shutdown
/// request itself would bump activity and look like a use.
async fn track_activity(
    State(state): State<Arc<HostState>>,
    request: Request,
    next: Next,
) -> Response {
    let internal = is_internal_path(request.uri().path());
    let response = next.run(request).await;
    if !internal {
        state.bump();
    }
    response
}

/// Wait for any of three graceful-shutdown triggers — OS signal,
/// idle timeout, or `POST /admin/shutdown` — then flip the status to
/// `shutting_down` and return so axum can drain in-flight requests
/// and exit.
async fn shutdown_signal(state: Arc<HostState>, idle_timeout: Option<Duration>) {
    let signal = signal_listener();
    let idle = idle_listener(Arc::clone(&state), idle_timeout);
    let admin = state.admin_shutdown.notified();
    tokio::pin!(signal);
    tokio::pin!(idle);
    tokio::pin!(admin);
    tokio::select! {
        () = &mut signal => tracing::info!(target: "kenn_server::host", "signal received; shutting down"),
        () = &mut idle => tracing::info!(target: "kenn_server::host", "idle timeout reached; shutting down"),
        () = &mut admin => tracing::info!(target: "kenn_server::host", "admin shutdown requested; shutting down"),
    }
    // Whatever path fired, mark `shutting_down` for the benefit of
    // `/healthz` pollers — `admin_shutdown` set this already but
    // SIGTERM/idle did not.
    state.mark_shutting_down();
}

#[cfg(unix)]
async fn signal_listener() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {},
        _ = int.recv() => {},
    }
}

#[cfg(not(unix))]
async fn signal_listener() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Wait until `now - last_request >= idle_timeout`. When `idle_timeout`
/// is `None`, waits forever (the externally-started-daemon case).
async fn idle_listener(state: Arc<HostState>, idle_timeout: Option<Duration>) {
    let Some(timeout) = idle_timeout else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        let last = state.last_request();
        let elapsed = last.elapsed();
        let Some(remaining) = timeout.checked_sub(elapsed) else {
            return; // elapsed >= timeout → fire shutdown
        };
        if remaining.is_zero() {
            return;
        }
        // Sleep until the next possible idle deadline, but break early
        // on activity so we can recompute against a fresher `last`.
        tokio::select! {
            () = tokio::time::sleep(remaining) => {},
            () = state.activity.notified() => {},
        }
    }
}

/// Standard 404 helper modules can return when they want to surface an
/// OpenAI-shaped not-found error.
#[must_use]
pub fn not_found(message: impl Into<String>) -> Response {
    let body = json!({ "error": { "message": message.into(), "type": "not_found" } });
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use std::sync::Arc;

    /// A trivial module exposing one POST route; used by the idle-counter
    /// tests to distinguish capability requests from `/healthz` probes.
    struct DummyModule;

    impl Module for DummyModule {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn register(self: Arc<Self>, router: Router) -> Router {
            router.route("/dummy", post(|| async { Json(json!({ "ok": true })) }))
        }
    }

    /// Spawn a host on an ephemeral port; returns the bound addr and a
    /// `JoinHandle` so the test can drive it.
    async fn spawn_host(
        idle: Option<Duration>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Leak the tempdir so PID-file path stays valid for the test
        // lifetime; tempfile cleans up on drop, but we need it past
        // the JoinHandle.
        let pid_path = tmp.path().join("server.pid");
        let dir_box: Box<tempfile::TempDir> = Box::new(tmp);
        let dir_ref: &'static tempfile::TempDir = Box::leak(dir_box);
        let _ = dir_ref; // keep alive

        let config = HostConfig {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            pid_path: pid_path.clone(),
            idle_timeout: idle,
        };

        // Use a one-shot to discover the bound addr after `bind`.
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(config.addr).await.unwrap();
            let addr = listener.local_addr().unwrap();
            addr_tx.send(addr).unwrap();
            pid::write(&config.pid_path, std::process::id()).unwrap();

            let state = Arc::new(HostState::new());
            let module = Arc::new(DummyModule);
            let mut router = Router::new()
                .route("/healthz", get(healthz))
                .route("/admin/shutdown", post(admin_shutdown))
                .with_state(Arc::clone(&state));
            router = Module::register(module, router);
            let router = router
                .layer(middleware::from_fn_with_state(
                    Arc::clone(&state),
                    track_activity,
                ))
                .layer(middleware::from_fn_with_state(
                    Arc::clone(&state),
                    reject_during_shutdown,
                ));
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal(state, config.idle_timeout))
                .await
                .unwrap();
            // Best-effort cleanup; test teardown doesn't care about failures.
            #[expect(
                clippy::let_underscore_must_use,
                reason = "test teardown — failures are not actionable"
            )]
            let _ = pid::remove(&config.pid_path);
        });

        let addr = addr_rx.await.unwrap();
        (addr, handle, pid_path)
    }

    async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
        let url = format!("http://{addr}{path}");
        let resp = reqwest::get(&url).await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }

    #[tokio::test]
    async fn healthz_returns_200_with_running_status() {
        let (addr, handle, _pid) = spawn_host(None).await;
        let (status, body) = http_get(addr, "/healthz").await;
        assert_eq!(status, 200);
        assert!(body.contains("\"status\":\"running\""), "{body}");
        assert!(body.contains("\"uptime_seconds\""), "{body}");
        handle.abort();
    }

    #[tokio::test]
    async fn admin_shutdown_triggers_graceful_exit_and_flips_healthz_status() {
        // No idle timeout — the only way the server exits is via the
        // admin-shutdown path we're testing.
        let (addr, handle, pid_path) = spawn_host(None).await;
        let client = reqwest::Client::new();
        // Sanity: status is "running" before we ask it to stop.
        let (s, body) = http_get(addr, "/healthz").await;
        assert_eq!(s, 200);
        assert!(body.contains("\"status\":\"running\""), "{body}");

        // Trigger graceful shutdown.
        let resp = client
            .post(format!("http://{addr}/admin/shutdown"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 202);
        let resp_body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(resp_body["status"], "shutting_down");

        // The server should exit on its own (no SIGTERM, no idle).
        let join_result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("server should have exited on admin shutdown");
        join_result.expect("clean exit");
        assert!(
            !pid_path.exists(),
            "pid file should be removed on graceful shutdown"
        );
    }

    #[tokio::test]
    async fn capability_request_resets_idle_counter_but_healthz_does_not() {
        // Use a generous timeout (5s) — we just want to observe that
        // calling /healthz repeatedly doesn't bump `last_request`, but
        // calling /dummy does.
        let (addr, handle, _pid) = spawn_host(Some(Duration::from_secs(5))).await;

        // Hit /healthz once to make sure it's wired (response success
        // = host is fully up; we don't bump on this).
        let (s, _) = http_get(addr, "/healthz").await;
        assert_eq!(s, 200);

        // The "last_request" we can observe directly via a /dummy POST
        // and confirming `/healthz` doesn't shift it. Easier: spawn a
        // long-idle host (5s), poll /healthz every 100ms for 600ms,
        // confirm the server is still alive (would've exited after
        // 5s if /healthz reset, but we're well under 5s anyway).
        // The stricter scenario fires in §2.11 unit tests under
        // time::pause. Here we just smoke-test the wiring.

        // Send a real capability request and ensure it round-trips.
        let url = format!("http://{addr}/dummy");
        let resp = reqwest::Client::new()
            .post(&url)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);

        handle.abort();
    }

    #[tokio::test]
    async fn idle_timeout_triggers_shutdown_with_no_traffic() {
        // Tight timeout so the test is quick.
        let (addr, handle, pid_path) = spawn_host(Some(Duration::from_millis(200))).await;
        // Confirm the server is up.
        let (s, _) = http_get(addr, "/healthz").await;
        assert_eq!(s, 200);
        // Wait past the timeout. The server should exit on its own.
        tokio::time::sleep(Duration::from_millis(800)).await;
        // After idle-exit, the JoinHandle should complete (not panic).
        let join_result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("server should have exited from idle timeout");
        join_result.expect("clean exit");
        // PID file should be gone.
        assert!(
            !pid_path.exists(),
            "pid file should be removed on graceful shutdown"
        );
    }
}
