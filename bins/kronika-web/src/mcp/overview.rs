//! `kronika_overview`: whole-window ranking, the entry point.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::heatmap;
use crate::config::Config;
use crate::route::HeatmapRequest;

use super::catalog::OverviewInput;
use super::semantics::{DecimalI64, mcp_error, mcp_structured};

pub(crate) fn call(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: OverviewInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };

    let request = HeatmapRequest {
        from: input.from,
        to: input.to,
        section: input.section,
        fields: input.fields,
        columns: 1,
        top: input.top as usize,
        labels: Vec::new(),
        group: Vec::new(),
        type_id: None,
    };

    let prepared = match heatmap::prepare(&config.data_root, request) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };

    let ranking = match prepared.rank_only(&|| false) {
        Ok(Some(ranking)) => ranking,
        Ok(None) => {
            return mcp_structured(
                json!({
                    "entities": [],
                    "totals_total": null,
                    "others_total": null,
                    "entity_count": DecimalI64(0),
                }),
                "no rows matched the requested window",
            );
        }
        Err(error) => return mcp_error(error.to_string()),
    };

    let entity_count = ranking.entity_count;
    let entities: Vec<Value> = ranking
        .entities
        .into_iter()
        .map(|entity| json!({ "key": entity.key, "total": entity.total }))
        .collect();
    let summary = format!(
        "{} ranked entities of {entity_count} recorded",
        entities.len()
    );

    mcp_structured(
        json!({
            "entities": entities,
            "totals_total": ranking.totals_total,
            "others_total": ranking.others_total,
            "entity_count": DecimalI64(i64::try_from(entity_count).unwrap_or(i64::MAX)),
        }),
        summary,
    )
}
