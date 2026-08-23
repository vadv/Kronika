//! Exact bounded MCP projection of recorded Event streams.

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use kronika_registry::{ColumnType, logical_section_name, registry};
use serde_json::{Map, Value, json};

use super::{ExpertFailure, ExpertPayload};
use crate::api::{self, EventPageError, EventPageRequest, EventSourceRequest};
use crate::mcp::State;

const EVENT_SECTIONS: &[&str] = &[
    "pg_log_errors",
    "pg_log_checkpoints",
    "pg_log_autovacuum",
    "pg_log_slow_queries",
    "pg_log_lock_waits",
    "pg_log_lifecycle",
    "pgbouncer_events",
];

const EVENT_LONG_TEXT_FIELDS: &[&str] = &[
    "pattern",
    "sample",
    "detail",
    "hint",
    "context",
    "statement",
    "message",
    "query_detail",
    "text",
];

pub(super) fn execute(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    let from = super::decimal_i64(args, "from_us")?;
    let to = super::decimal_i64(args, "to_us")?;
    super::bounded_window(from, to)?;
    validate_order(args)?;
    validate_direction(args)?;
    let requested_fields = super::strings(args, "fields", super::MAX_FIELDS)?;
    reject_long_text(&requested_fields)?;
    let sources = sources(args, &requested_fields)?;
    let find = optional_nonempty(args, "find")?;
    let cursor = optional_nonempty(args, "cursor")?;
    let page_size = super::usize_arg(
        args,
        "page_size",
        super::DEFAULT_PAGE_ROWS,
        1,
        super::MAX_PAGE_ROWS,
    )?;
    let page = api::read_event_page(
        &state.data_root,
        &EventPageRequest {
            from_us: from,
            to_us: to,
            sources,
            find,
            direction: super::order(args)?,
            page_size,
            value_bytes: super::data_budget(args)?,
            cursor,
        },
        cancelled,
    )
    .map_err(event_failure)?;
    if cancelled() {
        return Err(cancelled_failure());
    }
    let semantics = super::event_semantics()?;
    let returned = page.events.len();
    let has_more = page.next_cursor.is_some();
    Ok(ExpertPayload {
        anchor: json!({
            "hour_start_us": from
                .div_euclid(super::HOUR_US)
                .saturating_mul(super::HOUR_US)
                .to_string(),
            "requested_at_us": Value::Null,
            "selected_at_us": Value::Null,
            "segment_id": Value::Null,
            "active_wal_position": page.active_position.map(|value| value.to_string()),
        }),
        data: json!({
            "groups": [],
            "events": page.events,
            "semantics": semantics,
        }),
        page: super::page(
            returned,
            has_more,
            page.next_cursor,
            page.stop_reason.code(),
        ),
        warnings: page.warnings,
        summary: if has_more {
            format!("Returned {returned} recorded Event rows; another matching page is available.")
        } else {
            format!("Returned {returned} recorded Event rows.")
        },
    })
}

fn validate_order(args: &Map<String, Value>) -> Result<(), ExpertFailure> {
    match args.get("order") {
        None => Ok(()),
        Some(Value::String(order)) if order == "timestamp" => Ok(()),
        Some(_) => Err(super::failure(
            "invalid_parameter",
            "order must be timestamp",
            Some("order"),
            false,
        )),
    }
}

fn validate_direction(args: &Map<String, Value>) -> Result<(), ExpertFailure> {
    match args.get("direction") {
        None => Ok(()),
        Some(Value::String(direction)) if direction == "asc" || direction == "desc" => Ok(()),
        Some(_) => Err(super::failure(
            "invalid_parameter",
            "direction must be asc or desc",
            Some("direction"),
            false,
        )),
    }
}

fn sources(
    args: &Map<String, Value>,
    requested_fields: &[String],
) -> Result<Vec<EventSourceRequest>, ExpertFailure> {
    let requested = super::strings(args, "sources", EVENT_SECTIONS.len())?;
    let mut selected = if requested.is_empty() {
        EVENT_SECTIONS.iter().copied().collect::<HashSet<_>>()
    } else {
        let selected = requested.iter().map(String::as_str).collect::<HashSet<_>>();
        if selected.len() != requested.len() {
            return Err(super::failure(
                "invalid_parameter",
                "sources must not contain duplicates",
                Some("sources"),
                false,
            ));
        }
        selected
    };
    if let Some(unsupported) = selected
        .iter()
        .find(|source| !EVENT_SECTIONS.contains(source))
    {
        return Err(super::failure(
            "unsupported_source",
            format!("unsupported Event source {unsupported:?}"),
            Some("sources"),
            false,
        ));
    }
    validate_event_fields(requested_fields, &selected)?;
    EVENT_SECTIONS
        .iter()
        .filter(|source| selected.remove(**source))
        .map(|source| {
            Ok(EventSourceRequest {
                logical_name: (*source).to_owned(),
                fields: event_fields(source, requested_fields),
            })
        })
        .collect()
}

fn validate_event_fields(
    requested: &[String],
    selected: &HashSet<&str>,
) -> Result<(), ExpertFailure> {
    for field in requested {
        if !registry().iter().any(|layout| {
            logical_section_name(layout.type_id.get())
                .is_some_and(|name| selected.contains(name) && layout.column(field).is_some())
        }) {
            return Err(super::failure(
                "no_such_column",
                format!("no recorded Event layout has field {field:?}"),
                Some("fields"),
                false,
            ));
        }
    }
    Ok(())
}

fn event_fields(section: &str, requested: &[String]) -> Vec<String> {
    let layouts = registry().iter().filter(|layout| {
        logical_section_name(layout.type_id.get()).is_some_and(|name| name == section)
    });
    let layouts = layouts.collect::<Vec<_>>();
    if requested.is_empty() {
        let mut fields = Vec::new();
        for column in layouts.iter().flat_map(|layout| layout.columns) {
            if column.name != "ts"
                && column.ty != ColumnType::StrId
                && !fields.iter().any(|field| field == column.name)
            {
                fields.push(column.name.to_owned());
                if fields.len() == super::MAX_FIELDS {
                    break;
                }
            }
        }
        return fields;
    }
    requested
        .iter()
        .filter(|field| layouts.iter().any(|layout| layout.column(field).is_some()))
        .cloned()
        .collect()
}

fn reject_long_text(fields: &[String]) -> Result<(), ExpertFailure> {
    if fields
        .iter()
        .any(|field| EVENT_LONG_TEXT_FIELDS.contains(&field.as_str()))
    {
        return Err(super::failure(
            "text_field_requires_detail",
            "Event rows do not expose unbounded message, query, or statement text",
            Some("fields"),
            false,
        ));
    }
    Ok(())
}

fn optional_nonempty(
    args: &Map<String, Value>,
    name: &'static str,
) -> Result<Option<String>, ExpertFailure> {
    args.get(name)
        .map(|_value| super::string(args, name).map(ToOwned::to_owned))
        .transpose()
}

fn event_failure(error: EventPageError) -> ExpertFailure {
    match error {
        EventPageError::Api(error) => super::api_failure(error),
        EventPageError::Cancelled => cancelled_failure(),
        EventPageError::ScanLimit => super::failure(
            "scan_limit_exceeded",
            "the Event scan reached its physical row or decoded-cell limit",
            None,
            false,
        ),
        EventPageError::SegmentLimit => super::failure(
            "scan_budget_exceeded",
            "request intersects more than 64 segments",
            None,
            false,
        ),
        EventPageError::WarningLimit => super::failure(
            "warning_limit_exceeded",
            "the Event scan encountered more than 64 recorded store warnings",
            None,
            false,
        ),
        EventPageError::FirstRowTooLarge => super::first_row_too_large(),
        EventPageError::Semantics(error) => {
            super::failure("semantics_unreadable", error.to_string(), None, false)
        }
        EventPageError::InvalidSemantics(message) => {
            super::failure("semantics_unreadable", message, None, false)
        }
    }
}

fn cancelled_failure() -> ExpertFailure {
    super::failure("cancelled", "Event read was cancelled", None, true)
}
