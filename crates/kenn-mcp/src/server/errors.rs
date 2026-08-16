use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData;
use serde::Serialize;

use kenn_query::{QueryError, QueryErrorCode};

/// Map a query error onto JSON-RPC's numeric error space.
///
/// This lives in the transport, not on the error. What went wrong is a fact
/// about the query — and `QueryErrorCode::as_str` carries it as a stable string
/// that the CLI renders too. The *number* is a convention of one wire format,
/// and a second front end has no use for it.
///
/// Custom server errors live in [-32099, -32000]. Cursor errors map to `-32602`
/// per the MCP pagination spec (mcp-pagination-spec-alignment §2.1): both
/// "decoded but stale" and "couldn't decode" share the standard "Invalid
/// params" code, with the specific cause distinguished by the `kenn_subcode`
/// field in the error's `data` payload.
pub(super) const fn json_rpc_code(code: QueryErrorCode) -> i32 {
    match code {
        QueryErrorCode::StaleCursor | QueryErrorCode::InvalidInput => -32602,
        // EmbedderStarting and EmptySnapshot share the -32002
        // "service-unavailable" family with IndexUnavailable — agents already
        // branch on the string code in `data.code`, and an empty snapshot is
        // conceptually "the index exists but has no data to serve you."
        QueryErrorCode::IndexUnavailable
        | QueryErrorCode::EmbedderStarting
        | QueryErrorCode::EmptySnapshot
        | QueryErrorCode::EmbeddingUnavailable => -32002,
        QueryErrorCode::InternalError => -32603,
    }
}

pub(super) fn json_result<T: Serialize>(
    r: Result<T, QueryError>,
) -> Result<CallToolResult, ErrorData> {
    match r {
        Ok(v) => match Content::json(v) {
            Ok(c) => Ok(CallToolResult::success(vec![c])),
            Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
        },
        Err(e) => Err(into_error_data(&e)),
    }
}

fn into_error_data(e: &QueryError) -> ErrorData {
    let code = rmcp::model::ErrorCode(json_rpc_code(e.code));
    let mut payload = serde_json::Map::new();
    // `kenn_subcode` lets agents disambiguate kenn-specific causes
    // when the JSON-RPC code is shared (e.g. `-32602` for both stale
    // and malformed cursors per mcp-pagination-spec-alignment §2.1).
    payload.insert(
        "kenn_subcode".into(),
        serde_json::Value::String(e.code.as_str().into()),
    );
    if let Some(d) = e.data.clone() {
        payload.insert("data".into(), d);
    }
    ErrorData {
        code,
        message: e.message.clone().into(),
        data: Some(serde_json::Value::Object(payload)),
    }
}

#[cfg(test)]
mod tests {
    use super::json_rpc_code;
    use kenn_query::QueryErrorCode;

    /// The numbers moved here from the error type when queries left the MCP
    /// crate. Asserted at the boundary that owns them, so a query-layer test
    /// never has to know what `-32602` means.
    #[test]
    fn codes_map_to_their_json_rpc_families() {
        // Both cursor faults share "Invalid params" per the MCP pagination
        // spec; `kenn_subcode` distinguishes them.
        assert_eq!(json_rpc_code(QueryErrorCode::StaleCursor), -32602);
        assert_eq!(json_rpc_code(QueryErrorCode::InvalidInput), -32602);
        // The service-unavailable family: the index cannot serve you yet.
        for code in [
            QueryErrorCode::IndexUnavailable,
            QueryErrorCode::EmbedderStarting,
            QueryErrorCode::EmptySnapshot,
            QueryErrorCode::EmbeddingUnavailable,
        ] {
            assert_eq!(json_rpc_code(code), -32002, "{}", code.as_str());
        }
        assert_eq!(json_rpc_code(QueryErrorCode::InternalError), -32603);
    }
}
