//! Thin MCP adapter for the shared `PostgreSQL` Vacuum product.

use serde_json::{Map, Value, json};

use super::{
    PostgresqlFailure, PostgresqlPayload, State, collect, failure, fields, page_size, timestamp,
};
use crate::api::ValueStopReason;
use crate::route::{Route, VacuumRequest};

pub(super) fn execute(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    let from = timestamp(args, "from_us")?;
    let to = timestamp(args, "to_us")?;
    let request = VacuumRequest {
        from,
        to,
        at: to,
        fields: fields(args, &[])?,
        page_size: page_size(args)?,
    };
    let collected = collect(state, Route::Vacuum(request), cancelled)?;
    if collected.stop_reason != ValueStopReason::Complete {
        return Err(failure(
            "result_bound_exceeded",
            "the shared Vacuum product exceeds the retained result bound",
            Some("page_size"),
        ));
    }
    let product = collected
        .records
        .into_iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("vacuum"))
        .ok_or_else(|| {
            failure(
                "vacuum_product_unreadable",
                "the shared Vacuum producer returned no product record",
                None,
            )
        })?;
    let episodes = member(&product, "episodes")?;
    let returned = episodes.as_array().map_or(0, Vec::len);
    Ok(PostgresqlPayload {
        anchor: member(&product, "anchor")?,
        data: json!({
            "episodes": episodes,
            "semantics": member(&product, "semantics")?,
        }),
        page: member(&product, "page")?,
        warnings: member(&product, "warnings")?
            .as_array()
            .cloned()
            .ok_or_else(|| {
                failure(
                    "vacuum_product_unreadable",
                    "the shared Vacuum product warnings are not an array",
                    None,
                )
            })?,
        summary: format!("Returned {returned} Vacuum episode summaries."),
    })
}

fn member(product: &Value, name: &'static str) -> Result<Value, PostgresqlFailure> {
    product.get(name).cloned().ok_or_else(|| {
        failure(
            "vacuum_product_unreadable",
            format!("the shared Vacuum product has no {name}"),
            None,
        )
    })
}
