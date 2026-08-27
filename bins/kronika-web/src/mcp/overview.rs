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

    let section = input.section.clone();
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

    if ranking.entity_count == 0 {
        return empty_window_result(
            &section,
            (input.from, input.to),
            prepared.recorded_range(),
            &ranking.scan,
        );
    }

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

/// The empty-window response: a ranked shape plus where the section's rows sit.
fn empty_window_result(
    section: &str,
    window: (i64, i64),
    recorded: Option<(i64, i64)>,
    scan: &heatmap::ScanStats,
) -> CallToolResult {
    let summary = empty_window_summary(section, window, recorded, scan);
    mcp_structured(
        json!({
            "entities": [],
            "totals_total": null,
            "others_total": null,
            "entity_count": DecimalI64(0),
            "recorded_from": recorded.map(|(first, _last)| DecimalI64(first)),
            "recorded_to": recorded.map(|(_first, last)| DecimalI64(last)),
            "nearest_row_before": scan.nearest_before.map(DecimalI64),
            "nearest_row_after": scan.nearest_after.map(DecimalI64),
            "window_rows": DecimalI64(i64::try_from(scan.window_rows).unwrap_or(i64::MAX)),
        }),
        summary,
    )
}

/// Names where this section's recorded rows sit relative to an empty window.
pub(super) fn empty_window_summary(
    section: &str,
    window: (i64, i64),
    recorded: Option<(i64, i64)>,
    scan: &heatmap::ScanStats,
) -> String {
    let (from, to) = window;
    let Some((first, last)) = recorded else {
        return "The store holds no recorded segments.".to_owned();
    };
    if scan.window_rows > 0 {
        return format!(
            "{count} {section} rows sit inside the window, but none carries \
             a usable value in the requested fields; a wider window changes \
             nothing — `kronika_get_context` lists the section's other \
             fields.",
            count = scan.window_rows
        );
    }
    if to < first || from > last {
        return format!(
            "No {section} rows: the window lies outside the recorded \
             {first}..{last} microsecond range."
        );
    }
    let neighbours = match (scan.nearest_before, scan.nearest_after) {
        (Some(before), Some(after)) => {
            format!("the closest {section} rows the scan saw sit at {before} and {after}")
        }
        (Some(before), None) => format!("the closest {section} row the scan saw sits at {before}"),
        (None, Some(after)) => format!("the closest {section} row the scan saw sits at {after}"),
        (None, None) if scan.layouts_without_fields => {
            return format!(
                "No {section} rows ranked inside the window: the requested \
                 fields are not part of every recorded {section} layout — \
                 `kronika_get_context` names each layout's fields. The \
                 store records {first}..{last}."
            );
        }
        (None, None) => {
            return format!(
                "No {section} rows inside the window, and none nearby in the \
                 segments overlapping it; the store records {first}..{last}. \
                 Sections are written on their own intervals — retry with a \
                 much wider window."
            );
        }
    };
    format!(
        "No {section} rows ranked inside the window; {neighbours}. Sections \
         are written on their own intervals — widen the window to reach a \
         recorded row."
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
