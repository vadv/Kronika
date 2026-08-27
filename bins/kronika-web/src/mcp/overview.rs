//! `kronika_overview`: whole-window ranking for recorded section fields.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::heatmap;
use crate::config::Config;
use crate::route::{HeatmapRequest, MAX_HEATMAP_TOP};

use super::catalog::OverviewInput;
use super::semantics::{DecimalI64, bounded_limit, mcp_error, mcp_structured};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: OverviewInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    let top = match bounded_limit("top", input.top, MAX_HEATMAP_TOP) {
        Ok(top) => top,
        Err(error) => return error,
    };
    if input.fields.is_empty() || input.fields.len() > 4 {
        return mcp_error(format!(
            "fields must name 1 to 4 columns, got {}",
            input.fields.len()
        ));
    }
    for (index, field) in input.fields.iter().enumerate() {
        if input.fields[..index].contains(field) {
            return mcp_error(format!("fields names {field:?} twice"));
        }
    }
    if input.to < input.from {
        return mcp_error(format!(
            "to ({}) must not be before from ({})",
            input.to, input.from
        ));
    }

    let request = HeatmapRequest {
        from: input.from,
        to: input.to,
        section: input.section,
        fields: input.fields,
        columns: 1,
        top,
        labels: Vec::new(),
        group: Vec::new(),
        type_id: None,
    };

    let prepared = match heatmap::prepare(&config.data_root, request) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };

    let ranking = match prepared.rank_only(&|| cancelled()) {
        Ok(Some(ranking)) => ranking,
        Ok(None) => {
            return mcp_structured(
                json!({
                    "entities": [],
                    "totals_total": null,
                    "others_total": null,
                    "entity_count": DecimalI64(0),
                }),
                "No recorded rows matched the inclusive window.",
            );
        }
        Err(error) => return mcp_error(error.to_string()),
    };

    let entity_count = ranking.entity_count;
    let entities: Vec<Value> = ranking
        .entities
        .into_iter()
        .map(|entity| {
            json!({
                "identity": identity_object(entity.type_id, entity.identity),
                "total": entity.total,
            })
        })
        .collect();
    let summary = format!(
        "Returned {} of {entity_count} recorded identities.",
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

/// Recorded identity column names whose values a `kronika_find_*` filter
/// accepts under a different spelling. Emitting the finder's spelling
/// makes the overview -> find handoff a verbatim copy instead of a
/// rename the caller has to guess.
const IDENTITY_ALIASES: [(&str, &str); 2] = [("queryid", "query_id"), ("planid", "plan_id")];

/// Names an entity's identity values with the section's identity column
/// names from the registry (finder-accepted spellings where they differ),
/// so a ranked entity reads as `{"query_id": ..., "dbid": ...}` rather
/// than an unlabeled tuple. A registry/identity length mismatch falls
/// back to positional `value_N` names rather than dropping the values.
fn identity_object(type_id: u32, values: Vec<Value>) -> Value {
    let names = kronika_registry::contract(type_id)
        .map(|contract| contract.identity)
        .unwrap_or_default();
    let mut object = Map::new();
    for (index, value) in values.into_iter().enumerate() {
        let name = names.get(index).map_or_else(
            || format!("value_{index}"),
            |name| {
                IDENTITY_ALIASES
                    .iter()
                    .find(|(recorded, _)| recorded == name)
                    .map_or(*name, |(_, public)| *public)
                    .to_owned()
            },
        );
        object.insert(name, value);
    }
    Value::Object(object)
}
