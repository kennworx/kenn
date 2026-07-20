use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData;
use serde::Serialize;

use crate::error::McpError;

pub(super) fn json_result<T: Serialize>(
    r: Result<T, McpError>,
) -> Result<CallToolResult, ErrorData> {
    match r {
        Ok(v) => match Content::json(v) {
            Ok(c) => Ok(CallToolResult::success(vec![c])),
            Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
        },
        Err(e) => Err(into_error_data(&e)),
    }
}

fn into_error_data(e: &McpError) -> ErrorData {
    let code = rmcp::model::ErrorCode(e.code.json_rpc_code());
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
