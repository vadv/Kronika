use rmcp::ErrorData;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};

use super::{State, catalog};

const NOT_WIRED: &str = "This historical surface is not wired yet.";
const _: () = assert!(NOT_WIRED.len() <= super::TEXT_SUMMARY_BYTES);

pub(super) fn dispatch(
    _state: &State,
    request: CallToolRequestParams,
) -> Result<CallToolResult, ErrorData> {
    if catalog::find(request.name.as_ref()).is_none() {
        return Err(ErrorData::invalid_params("tool not found", None));
    }
    let structured = serde_json::json!({
        "status": "error",
        "error": {
            "code": "not_wired",
            "message": NOT_WIRED,
        }
    });
    let mut result = CallToolResult::structured_error(structured);
    result.content = vec![ContentBlock::text(NOT_WIRED)];
    Ok(result)
}
