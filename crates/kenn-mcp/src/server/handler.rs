use std::sync::Arc;

use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{tool_handler, ErrorData, ServerHandler};

use super::core::{KennMcpServer, AGENT_GUIDE};

#[tool_handler]
impl ServerHandler for KennMcpServer {
    /// Per-tool observability boundary. We define `call_tool` ourselves so the
    /// `#[tool_handler]` macro skips its own generated version (it only emits
    /// one when the impl block has no `call_tool`) and instead routes through
    /// this wrapper, which:
    /// 1. opens one `tracing` span per call keyed by the requested tool name —
    ///    emitted to stderr (never stdout, which the stdio transport owns for
    ///    JSON-RPC); the span's open→close interval is the tool duration;
    /// 2. records a `metrics`-crate counter + duration histogram. The facade is
    ///    a no-op until an exporter is installed, so "metrics later" is a config
    ///    flip with no change here.
    ///
    /// Delegation mirrors the macro expansion exactly: build a
    /// `ToolCallContext` and hand it to `Self::tool_router()`.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        use tracing::Instrument;

        let tool = request.name.to_string();
        let span = tracing::info_span!("mcp.tool", tool = %tool);
        let start = std::time::Instant::now();

        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let result = Self::tool_router().call(tcc).instrument(span).await;

        let elapsed = start.elapsed().as_secs_f64();
        metrics::counter!("mcp.tool.calls", "tool" => tool.clone()).increment(1);
        metrics::histogram!("mcp.tool.duration_seconds", "tool" => tool).record(elapsed);

        result
    }

    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2024_11_05;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        // Injected into the session at handshake. The text lives in the sibling
        // `kenn-agent.md` asset (data, not code) — the code graph + the findings
        // store + the directive workflow, so the agent is told the knowledge
        // layer exists, not just the code graph.
        info.instructions = Some(AGENT_GUIDE.into());
        info
    }

    /// Capture the client's declared capabilities at `initialize`
    /// time. Currently we only consume the `roots` capability and
    /// its `list_changed` sub-flag — both gate the post-handshake
    /// roots-rebind path (mcp-roots-discovery §5/§7).
    ///
    /// Falls through to rmcp's default after recording the flags,
    /// preserving the standard protocol-version / server-info /
    /// instructions response built from `get_info()`.
    async fn initialize(
        &self,
        request: rmcp::model::InitializeRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, ErrorData> {
        use std::sync::atomic::Ordering;

        let roots = request.capabilities.roots.as_ref();
        self.state
            .client_supports_roots
            .store(roots.is_some(), Ordering::Relaxed);
        self.state.client_supports_roots_list_changed.store(
            roots.and_then(|r| r.list_changed).unwrap_or(false),
            Ordering::Relaxed,
        );

        // `InitializeResult` is `#[non_exhaustive]` — build the body
        // from `ServerInfo` (which is also the shape `get_info()`
        // returns) and let `Into` coerce into the result struct.
        Ok(self.get_info())
    }

    /// Fires after the client sends `notifications/initialized`.
    /// If the client declared `roots` and the operator didn't pin a
    /// `--workspace` flag, query `roots/list` and rebind if the host
    /// reports a different workspace than our tentative bind. See
    /// mcp-roots-discovery §5.
    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        if !self
            .state
            .client_supports_roots
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        if self.state.workspace_source().is_permanent() {
            return;
        }
        let state = Arc::clone(&self.state);
        let peer = context.peer.clone();
        tokio::spawn(async move {
            crate::indexing::resolve_roots_and_maybe_rebind(state, peer).await;
        });
    }

    /// Fires when the client signals its roots list changed. Same
    /// rebind path as the initial post-handshake resolution. See
    /// mcp-roots-discovery §7.
    async fn on_roots_list_changed(
        &self,
        context: rmcp::service::NotificationContext<rmcp::RoleServer>,
    ) {
        if !self
            .state
            .client_supports_roots_list_changed
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            // Client declared `roots` without `listChanged: true` but
            // sent the notification anyway. Spec is permissive; we
            // could rebind, but staying consistent with the declared
            // capability is the safer default.
            return;
        }
        if self.state.workspace_source().is_permanent() {
            return;
        }
        let state = Arc::clone(&self.state);
        let peer = context.peer.clone();
        tokio::spawn(async move {
            crate::indexing::resolve_roots_and_maybe_rebind(state, peer).await;
        });
    }
}
