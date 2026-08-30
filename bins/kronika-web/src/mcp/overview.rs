//! `kronika_overview`: thin MCP adapter for the shared Heatmap batch.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::heatmap::{
    HeatmapBatchQuery, HeatmapItemQuery, HeatmapView, NormalizedRanking, prepare_batch,
};
use crate::config::Config;
use crate::route::MAX_QUERY_BYTES;

use super::catalog::{OVERVIEW_TOOL, OverviewInput, OverviewRankingInput};
use super::semantics::{
    arguments_within_budget, invalid_arguments, mcp_error, mcp_error_indexed,
    mcp_error_indexed_with, mcp_structured,
};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    if let Err((index, message)) = check_request_budget(&arguments) {
        return mcp_error_indexed(format!("rankings[{index}]: {message}"), index);
    }
    if let Some(rankings) = arguments.get("rankings").and_then(Value::as_array) {
        for (index, ranking) in rankings.iter().enumerate() {
            if let Err(error) = serde_json::from_value::<OverviewRankingInput>(ranking.clone()) {
                return mcp_error_indexed(
                    format!("invalid {OVERVIEW_TOOL} rankings[{index}]: {error}"),
                    index,
                );
            }
        }
    }
    let input: OverviewInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return invalid_arguments(
                OVERVIEW_TOOL,
                "from, to, and a nonempty rankings array are required; each ranking contains section, 1-4 fields, and optional top",
                error,
            );
        }
    };
    if input.rankings.is_empty() {
        return mcp_error_indexed("rankings[0]: rankings must not be empty", 0);
    }
    let range = match super::time::resolve_range(&input.from, &input.to) {
        Ok(range) => range,
        Err(error) => return mcp_error_indexed(format!("rankings[0]: {error}"), 0),
    };
    let mut items = Vec::with_capacity(input.rankings.len());
    for (index, ranking) in input.rankings.into_iter().enumerate() {
        let top = match usize::try_from(ranking.top) {
            Ok(top) => top,
            Err(_error) => {
                return mcp_error_indexed(
                    format!("rankings[{index}]: top does not fit this platform"),
                    index,
                );
            }
        };
        items.push(HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: ranking.section,
                fields: ranking.fields,
                top,
            },
            view: HeatmapView::RankingOnly,
        });
    }
    let prepared = match prepare_batch(&config.data_root, HeatmapBatchQuery { range, items }) {
        Ok(prepared) => prepared,
        Err(error) => {
            return mcp_error_indexed_with(
                error.to_string(),
                error.ranking_index(),
                error.valid_options().to_vec(),
            );
        }
    };
    let result = match prepared.execute(&|| cancelled()) {
        Ok(result) => result,
        Err(error) => {
            return mcp_error_indexed_with(
                error.to_string(),
                error.ranking_index(),
                error.valid_options().to_vec(),
            );
        }
    };
    let returned = result.results.len();
    let structured = match serde_json::to_value(result) {
        Ok(value) => value,
        Err(error) => return mcp_error(format!("could not encode overview result: {error}")),
    };
    mcp_structured(
        structured,
        format!("Returned {returned} ordered stored-data rankings."),
    )
}

fn check_request_budget(arguments: &Map<String, Value>) -> Result<(), (usize, String)> {
    if arguments_within_budget(arguments) {
        return Ok(());
    }
    let rankings = arguments
        .get("rankings")
        .and_then(Value::as_array)
        .ok_or_else(|| (0, request_overflow_message()))?;
    let mut without_rankings = arguments.clone();
    without_rankings.insert("rankings".to_owned(), Value::Array(Vec::new()));
    let mut used = serde_json::to_vec(&Value::Object(without_rankings))
        .map_err(|error| (0, format!("could not measure arguments: {error}")))?
        .len();
    if used > MAX_QUERY_BYTES {
        return Err((0, request_overflow_message()));
    }
    for (index, ranking) in rankings.iter().enumerate() {
        if index > 0 {
            used = used
                .checked_add(1)
                .ok_or_else(|| (index, request_overflow_message()))?;
        }
        used = used
            .checked_add(serde_json::to_vec(ranking).map_or(0, |bytes| bytes.len()))
            .ok_or_else(|| (index, request_overflow_message()))?;
        if used > MAX_QUERY_BYTES {
            return Err((index, request_overflow_message()));
        }
    }
    Err((rankings.len().saturating_sub(1), request_overflow_message()))
}

fn request_overflow_message() -> String {
    format!(
        "overview arguments exceed {MAX_QUERY_BYTES} encoded bytes; split rankings into several calls or reduce top"
    )
}
