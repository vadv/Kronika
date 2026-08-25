//! Complete LLM-facing descriptors for Kronika's approved MCP tools.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use crate::product::top_activity::{metric_definitions, surface_definitions};

const I64_PATTERN: &str = "^(0|-[1-9][0-9]{0,18}|[1-9][0-9]{0,18})$";
const U64_PATTERN: &str = "^(0|[1-9][0-9]{0,19})$";
const RELATION_LEVELS: [(&str, &str); 4] = [
    (
        "object",
        "Rank exact table or index identities; the relation-surface default.",
    ),
    (
        "schema",
        "Combine member objects by recorded database and schema names.",
    ),
    (
        "database",
        "Combine member objects by recorded database name.",
    ),
    (
        "tablespace",
        "Combine member objects by the exact recorded nullable tablespace-name value; null is a distinct group key.",
    ),
];
const TOP_VALUES: [(u64, &str); 4] = [
    (10, "Return at most the first 10 ranked rows."),
    (
        25,
        "Return at most the first 25 ranked rows; the product default.",
    ),
    (50, "Return at most the first 50 ranked rows."),
    (
        100,
        "Return at most the first 100 ranked rows; the largest shipped choice.",
    ),
];

/// Return the complete descriptor set in discovery-first order.
pub(crate) fn tools() -> Vec<Tool> {
    vec![top_activity_tool(), activity_tool()]
}

fn annotations() -> ToolAnnotations {
    ToolAnnotations::new().read_only(true).open_world(false)
}

fn schema(value: Value) -> Arc<JsonObject> {
    Arc::new(object(value))
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            unreachable!("schema constructors always produce objects")
        }
    }
}

fn activity_tool() -> Tool {
    Tool::new(
        "kronika_read_postgresql_activity",
        "Returns one page of recorded PostgreSQL backend rows from the latest Activity observation in the UTC hour containing required `at`, with `observed_at <= at`; PostgreSQL is not queried. Use `kronika_find_top_activity` (Find top load across system and PostgreSQL) for broad ranked interval discovery, then Activity for filtered rows at a selected observation. Use Activity for recorded backend state and wait-event fields, query preview, identifiers, transaction-ID ages, timestamps, and durations; use the Locks product surface for lock-wait relationships and the Statements product surface for cumulative query workload. All retained rows are eligible unless filtered. Filtering precedes deterministic sorting and shared pagination; `page_size` defaults to 200, and `next_cursor` continues the same result. Tool errors omit structured content and carry one code/message text item; stable codes are `invalid_arguments`, `activity_read_failed`, `result_too_large`, `cancelled`, and `deadline_exceeded`.",
        schema(activity_input_schema()),
    )
    .with_title("Read recorded PostgreSQL backend activity")
    .with_raw_output_schema(schema(activity_output_schema()))
    .with_annotations(annotations())
}

fn activity_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Select, filter, sort, and page one recorded Activity observation.",
        "properties": {
            "at": {
                "type": "string",
                "pattern": I64_PATTERN,
                "maxLength": 20,
                "description": "Required signed i64 Unix microseconds as canonical decimal text. Selects the latest same-hour Activity observation with `observed_at <= at`; no default. Every i64 value is accepted, with containing-hour bounds computed without signed overflow."
            },
            "filter": {
                "$ref": "#/$defs/Filter",
                "description": "Optional OR array of flat Activity clauses. Omit to match every row; an empty array matches none. Supplied fields in each clause are ANDed."
            },
            "sort": {
                "type": "string",
                "default": "query_duration_ms",
                "description": "Primary sort key; defaults to `query_duration_ms`. Numeric keys are numeric, text keys use case-sensitive code-point order, nulls are last in either direction, and empty text is non-null.",
                "oneOf": activity_sort_choices()
            },
            "direction": {
                "type": "string",
                "default": "desc",
                "description": "Primary direction; defaults to `desc`. Nulls stay last. When the effective pair is `query_duration_ms`/`desc`, equal primary values use `transaction_duration_ms` descending with null last, then PID ascending. Every other sort/direction pair uses PID ascending.",
                "oneOf": [
                    {
                        "const": "asc",
                        "description": "Non-null primary values ascending; nulls last."
                    },
                    {
                        "const": "desc",
                        "description": "Non-null primary values descending; nulls last."
                    }
                ]
            },
            "page_size": {
                "type": "integer",
                "minimum": 1,
                "maximum": 5000,
                "default": 200,
                "description": "Maximum whole rows after filter and sort; defaults to 200. Whole-row fitting may return fewer with `next_cursor`."
            },
            "cursor": {
                "type": "string",
                "minLength": 1,
                "maxLength": 4096,
                "description": "Opaque `next_cursor` continuation. Reuse it with unchanged at, filter, sort, direction, and page size; changed, invalid, or expired bindings are rejected."
            }
        },
        "required": ["at"],
        "$defs": activity_input_definitions()
    })
}

fn activity_sort_choices() -> Vec<Value> {
    [
        ("pid", "Recorded backend PID, ordered numerically."),
        (
            "database",
            "Nullable database name (`datname`), ordered lexically.",
        ),
        (
            "role",
            "Nullable PostgreSQL role name (`usename`), ordered lexically.",
        ),
        (
            "query_preview",
            "Nullable returned query preview, ordered lexically; at most 161 Unicode scalar values, including `…` when this surface adds it.",
        ),
        (
            "query_duration_ms",
            "Active-query wall-clock milliseconds; null outside exact `active` or with unusable `query_start`.",
        ),
        (
            "transaction_duration_ms",
            "Transaction wall-clock milliseconds from usable `xact_start`; otherwise null.",
        ),
        (
            "application",
            "Application name (`application_name`), ordered lexically; empty is a value.",
        ),
        (
            "client",
            "Client host text (`client_addr`), ordered lexically rather than as an IP; empty is a value.",
        ),
        (
            "state",
            "Nullable open overall-state string, ordered lexically.",
        ),
        (
            "wait_type",
            "Nullable open wait-event class (`wait_event_type`), ordered lexically.",
        ),
        (
            "wait_event",
            "Nullable open wait-event name, ordered lexically.",
        ),
        (
            "backend_type",
            "Open backend-type string, ordered lexically.",
        ),
    ]
    .into_iter()
    .map(|(value, description)| json!({ "const": value, "description": description }))
    .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "the frozen Activity v6 schema stays together as one reviewable contract"
)]
fn activity_input_definitions() -> JsonObject {
    object(json!({
        "Pattern": {
            "type": "string",
            "minLength": 1,
            "maxLength": 256,
            "description": "Leading and trailing whitespace is ignored; a whitespace-only value is invalid. After trimming, matching is case-insensitive and unanchored. Without wildcards the pattern is a substring. `?` matches exactly one Unicode scalar value; `*` matches zero or more Unicode scalar values, including line terminators; every other scalar value is literal."
        },
        "Patterns": {
            "type": "array",
            "minItems": 1,
            "maxItems": 8,
            "items": { "$ref": "#/$defs/Pattern" }
        },
        "TextMatch": {
            "type": "object",
            "additionalProperties": false,
            "minProperties": 1,
            "maxProperties": 2,
            "description": "At least one nonempty list is required; when both are present, both must match.",
            "properties": {
                "any_of": {
                    "$ref": "#/$defs/Patterns",
                    "description": "The predicate target must match at least one pattern."
                },
                "all_of": {
                    "$ref": "#/$defs/Patterns",
                    "description": "Every pattern must match the predicate target."
                }
            }
        },
        "PidMatch": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "any_of": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "uniqueItems": true,
                    "items": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 2_147_483_647
                    },
                    "description": "Recorded PID must equal one listed positive i32."
                }
            },
            "required": ["any_of"]
        },
        "QueryId": {
            "type": "string",
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Canonical signed i64 Query ID as decimal text; parsed value must fit i64."
        },
        "QueryIdMatch": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "any_of": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "uniqueItems": true,
                    "items": { "$ref": "#/$defs/QueryId" },
                    "description": "Recorded Query ID must equal one listed signed i64."
                }
            },
            "required": ["any_of"]
        },
        "Clause": {
            "type": "object",
            "additionalProperties": false,
            "minProperties": 1,
            "maxProperties": 8,
            "description": "All supplied fields must match the same row; null targets do not match; at most eight listed values are allowed in total.",
            "properties": {
                "text": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "For each pattern independently, matches any one of `query_preview`, `datname`, `usename`, `application_name`, `client_addr`, `state`, `wait_event_type`, or `wait_event`; different `all_of` patterns may match different listed fields in the same row. PID, Query ID, and backend type are excluded."
                },
                "pid": {
                    "$ref": "#/$defs/PidMatch",
                    "description": "Exact recorded backend PID alternatives."
                },
                "query_id": {
                    "$ref": "#/$defs/QueryIdMatch",
                    "description": "Exact recorded Query ID alternatives."
                },
                "database": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Nullable database name (`datname`)."
                },
                "role": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Nullable PostgreSQL role name (`usename`)."
                },
                "application": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Application name (`application_name`)."
                },
                "client": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Client host text (`client_addr`); empty is a value."
                },
                "backend_type": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Open backend-type string."
                },
                "state": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Nullable open overall state, independent of the wait-event pair."
                },
                "wait_type": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Nullable open `wait_event_type`, independent of state."
                },
                "wait_event": {
                    "$ref": "#/$defs/TextMatch",
                    "description": "Nullable open `wait_event`."
                }
            }
        },
        "Filter": {
            "type": "array",
            "maxItems": 18,
            "items": { "$ref": "#/$defs/Clause" },
            "description": "Activity clauses combined with OR; an empty array matches no rows."
        }
    }))
}

fn activity_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Whole-row page from one Activity observation after filter and deterministic sort. When the effective pair is `query_duration_ms`/`desc`, ties use transaction duration descending then PID ascending; every other pair uses PID ascending. Nulls are last.",
        "properties": {
            "requested_at": {
                "$ref": "#/$defs/Timestamp",
                "description": "Requested `at` as canonical signed i64 Unix microseconds."
            },
            "observed_at": {
                "$ref": "#/$defs/NullableTimestamp",
                "description": "Greatest compatible Activity observation in the requested UTC hour at or before `requested_at`; null only when no such observation exists, and retained when the filter matches no rows."
            },
            "rows": {
                "type": "array",
                "maxItems": 5000,
                "items": { "$ref": "#/$defs/Row" },
                "description": "Whole matching rows in this page from `observed_at` in page order; empty with no observation or match."
            },
            "next_cursor": {
                "type": ["string", "null"],
                "minLength": 1,
                "maxLength": 4096,
                "description": "Nonempty opaque first-unreturned-row cursor, or null when the pinned result is exhausted."
            }
        },
        "required": ["requested_at", "observed_at", "rows", "next_cursor"],
        "$defs": activity_output_definitions()
    })
}

fn activity_output_definitions() -> JsonObject {
    let mut definitions = object(json!({
        "Timestamp": {
            "type": "string",
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 Unix microseconds as canonical decimal text."
        },
        "NullableTimestamp": {
            "type": ["string", "null"],
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 Unix microseconds as canonical decimal text, or null."
        },
        "NullableI64": {
            "type": ["string", "null"],
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 canonical decimal text, or null."
        },
        "NullableText": {
            "type": ["string", "null"],
            "description": "Recorded text or null."
        }
    }));
    definitions.insert(
        "NullableDuration".to_owned(),
        json!({
            "type": ["number", "null"],
            "minimum": 0,
            "description": "Finite nonnegative fractional wall-clock milliseconds or null; timestamp differences use checked wider integer arithmetic before conversion, and null differs from zero."
        }),
    );
    definitions.insert("Row".to_owned(), activity_row_schema());
    definitions
}

#[expect(
    clippy::too_many_lines,
    reason = "the frozen Activity v6 row schema stays together as one reviewable contract"
)]
fn activity_row_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Normalized backend row from the selected observation. PID is its observation-local identity; no physical locator is exposed.",
        "properties": {
            "observed_at": {
                "$ref": "#/$defs/Timestamp",
                "description": "Row observation timestamp; equals result-level `observed_at`."
            },
            "pid": {
                "type": "integer",
                "minimum": 1,
                "maximum": 2_147_483_647,
                "description": "PostgreSQL backend PID and identity within the selected observation."
            },
            "leader_pid": {
                "type": ["integer", "null"],
                "minimum": 1,
                "maximum": 2_147_483_647,
                "description": "Leader PID for a parallel query worker; on PostgreSQL 16–18, also the leader apply-worker PID for a parallel apply worker. Null for a leader or nonparticipant, PostgreSQL null, or Kronika stored layout `1_001_001` (PG10–12). It is not a parent or blocker PID."
            },
            "datid": {
                "type": ["integer", "null"],
                "minimum": 0,
                "maximum": 4_294_967_295_u64,
                "description": "Recorded database OID; null when PostgreSQL returned null or because Kronika stored layouts `1_001_001`/`1_001_002` (PG10–13) omit the field."
            },
            "datname": {
                "$ref": "#/$defs/NullableText",
                "description": "Database name, or null when PostgreSQL reported none."
            },
            "usename": {
                "$ref": "#/$defs/NullableText",
                "description": "PostgreSQL role name, or null when none was reported."
            },
            "application_name": {
                "type": "string",
                "description": "Application name; source null is stored as empty text."
            },
            "client_addr": {
                "type": "string",
                "description": "Text from `host(pg_stat_activity.client_addr)`, without a port. Source null is empty text, shared by socket clients and processes without a client address."
            },
            "backend_type": {
                "type": "string",
                "description": "Open backend-type string; values vary by PostgreSQL version and extensions."
            },
            "state": {
                "$ref": "#/$defs/NullableText",
                "description": "Open overall-state string, independent of wait fields; null when unavailable."
            },
            "wait_event_type": {
                "$ref": "#/$defs/NullableText",
                "description": "Open wait-event class at the observation, or null when none; independent of state."
            },
            "wait_event": {
                "$ref": "#/$defs/NullableText",
                "description": "Open event name within `wait_event_type`, or null when none."
            },
            "query_preview": {
                "type": ["string", "null"],
                "maxLength": 161,
                "description": "Current query for exact `active`, otherwise the most recent query, or null. Keeps the first 160 Unicode scalar values and appends `…` only when this surface cuts collector-retained text; upstream shortening is not marked, and the maximum is 161 scalar values."
            },
            "query_id": {
                "$ref": "#/$defs/NullableI64",
                "description": "Signed ID of the current query for exact `active`, otherwise the most recent query; null when PostgreSQL returned no query ID or because Kronika PG10–13 stored layouts omit the field."
            },
            "backend_xid_age": {
                "$ref": "#/$defs/NullableI64",
                "description": "`backend_xid` age as a transaction count, or null without an assigned transaction ID."
            },
            "backend_xmin_age": {
                "$ref": "#/$defs/NullableI64",
                "description": "Backend xmin-horizon age as a transaction count, or null when unavailable."
            },
            "backend_start": {
                "$ref": "#/$defs/Timestamp",
                "description": "Backend start in Unix microseconds; required in every PG10–18 Activity layout."
            },
            "xact_start": {
                "$ref": "#/$defs/NullableTimestamp",
                "description": "Transaction start in Unix microseconds, or null outside a transaction or when unavailable."
            },
            "query_start": {
                "$ref": "#/$defs/NullableTimestamp",
                "description": "Current-query start when active, otherwise most-recent-query start, in Unix microseconds; null when unavailable."
            },
            "state_change": {
                "$ref": "#/$defs/NullableTimestamp",
                "description": "Last overall-state change in Unix microseconds, or null when unavailable."
            },
            "backend_age_ms": {
                "$ref": "#/$defs/NullableDuration",
                "description": "`(observed_at - backend_start) / 1000`; null for a missing, non-positive, or later start."
            },
            "query_duration_ms": {
                "$ref": "#/$defs/NullableDuration",
                "description": "`(observed_at - query_start) / 1000` only for exact `active`; null for another state or a missing, non-positive, or later start."
            },
            "transaction_duration_ms": {
                "$ref": "#/$defs/NullableDuration",
                "description": "`(observed_at - xact_start) / 1000`; null for a missing, non-positive, or later start."
            },
            "state_duration_ms": {
                "$ref": "#/$defs/NullableDuration",
                "description": "`(observed_at - state_change) / 1000`; null for missing state, exact `idle`, or a missing, non-positive, or later start. Transactional-idle states retain it."
            }
        },
        "required": [
            "observed_at", "pid", "leader_pid", "datid", "datname", "usename",
            "application_name", "client_addr", "backend_type", "state",
            "wait_event_type", "wait_event", "query_preview", "query_id",
            "backend_xid_age", "backend_xmin_age", "backend_start", "xact_start",
            "query_start", "state_change", "backend_age_ms", "query_duration_ms",
            "transaction_duration_ms", "state_duration_ms"
        ]
    })
}

fn scalar_definitions() -> JsonObject {
    object(json!({
        "Timestamp": {
            "type": "string",
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 Unix microseconds as canonical decimal text."
        },
        "NullableTimestamp": {
            "type": ["string", "null"],
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 Unix microseconds as canonical decimal text, or null."
        },
        "NullableI64": {
            "type": ["string", "null"],
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 canonical decimal text, or null."
        },
        "I64": {
            "type": "string",
            "pattern": I64_PATTERN,
            "maxLength": 20,
            "description": "Signed i64 as canonical decimal text; the parsed value must fit i64."
        },
        "U64": {
            "type": "string",
            "pattern": U64_PATTERN,
            "maxLength": 20,
            "description": "Unsigned u64 as canonical decimal text; the parsed value must fit u64."
        },
        "NullableText": {
            "type": ["string", "null"],
            "description": "Recorded display text or null; null differs from empty text."
        }
    }))
}

#[derive(Debug)]
struct TopBranch {
    surface: &'static str,
    surface_description: &'static str,
    default_metric: &'static str,
    intervals: usize,
    relation_levels: bool,
    metrics: Vec<MetricChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MetricChoice {
    name: &'static str,
    description: &'static str,
}

fn top_branches() -> Vec<TopBranch> {
    surface_definitions()
        .iter()
        .map(|surface| TopBranch {
            surface: surface.surface.as_str(),
            surface_description: surface.description,
            default_metric: surface.default_metric.as_str(),
            intervals: surface.intervals,
            relation_levels: matches!(
                surface.surface.as_str(),
                "postgresql_tables" | "postgresql_indexes"
            ),
            metrics: metric_definitions()
                .iter()
                .filter(|metric| metric.surface == surface.surface)
                .map(|metric| MetricChoice {
                    name: metric.metric.as_str(),
                    description: metric.description,
                })
                .collect(),
        })
        .collect()
}

fn top_activity_tool() -> Tool {
    Tool::new(
        "kronika_find_top_activity",
        "Primary discovery entry for recorded system and PostgreSQL load. Returns one complete ranked result for one exact UTC hour, one shipped semantic surface, and one surface-specific metric, with interval cells, whole-hour values, Total, and Other. Use stable surface, metric, and entity identities to continue in a targeted product surface: for example, disk-heavy PostgreSQL activity can guide a Statements view sorted or filtered by disk work, then the selected statement identity can guide related recorded Logs when those product surfaces are available. PostgreSQL and live services are not queried. `top` defaults to 25. Successful calls return one structured result. Tool errors omit structured content and carry one code/message text item; stable codes are `invalid_arguments`, `heatmap_read_failed`, `result_too_large`, `cancelled`, and `deadline_exceeded`.",
        schema(top_input_schema()),
    )
    .with_title("Find top load across system and PostgreSQL")
    .with_raw_output_schema(schema(top_output_schema()))
    .with_annotations(annotations())
}

fn top_input_schema() -> Value {
    let branches = top_branches();
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Select one recorded UTC-hour activity ledger using semantic product inputs.",
        "properties": {
            "hour": {
                "type": "string",
                "pattern": I64_PATTERN,
                "maxLength": 20,
                "description": "Required exact UTC-hour start as canonical signed i64 Unix microseconds. It must be divisible by 3600000000; no rounding and no default. The inclusive result window ends at hour + 3599999999, computed without signed overflow."
            },
            "surface": {
                "$ref": "#/$defs/Surface",
                "description": "Required shipped product ledger whose entities, metrics, grouping, interval count, and default metric are selected."
            },
            "metric": {
                "type": "string",
                "description": "Optional surface-specific semantic metric. Omission uses that surface's documented default. Exact units and formulas are returned in definition. Block-size and clock-rate conversions use one latest usable positive recorded value at or before hour_end for the complete result; without one, raw values use count/count_per_second units."
            },
            "level": {
                "$ref": "#/$defs/Level",
                "description": "Optional relation aggregation level, accepted only for postgresql_tables and postgresql_indexes; defaults to object."
            },
            "top": {
                "$ref": "#/$defs/Top",
                "default": 25,
                "description": "Maximum ranked rows returned; defaults to 25. It is a top-K aggregation limit, not a page size."
            }
        },
        "required": ["hour", "surface"],
        "allOf": top_input_conditions(&branches),
        "$defs": top_input_definitions(&branches)
    })
}

fn top_input_conditions(branches: &[TopBranch]) -> Vec<Value> {
    branches
        .iter()
        .map(|branch| {
            let metric = json!({
                "type": "string",
                "default": branch.default_metric,
                "oneOf": branch.metrics.iter().map(metric_choice_schema).collect::<Vec<_>>()
            });
            let mut properties = Map::from_iter([("metric".to_owned(), metric)]);
            let mut then = Map::new();
            if branch.relation_levels {
                properties.insert(
                    "level".to_owned(),
                    json!({ "$ref": "#/$defs/Level", "default": "object" }),
                );
            } else {
                then.insert("not".to_owned(), json!({ "required": ["level"] }));
            }
            then.insert("properties".to_owned(), Value::Object(properties));
            json!({
                "if": {
                    "properties": { "surface": { "const": branch.surface } },
                    "required": ["surface"]
                },
                "then": then
            })
        })
        .collect()
}

fn metric_choice_schema(choice: &MetricChoice) -> Value {
    json!({
        "const": choice.name,
        "description": format!(
            "{} Exact units, conversion, null behavior, and ranking formula are returned in definition.",
            choice.description
        )
    })
}

fn top_input_definitions(branches: &[TopBranch]) -> JsonObject {
    object(json!({
        "Surface": {
            "type": "string",
            "oneOf": branches.iter().map(|branch| json!({
                "const": branch.surface,
                "description": branch.surface_description
            })).collect::<Vec<_>>()
        },
        "Top": {
            "type": "integer",
            "oneOf": TOP_VALUES.into_iter().map(|(value, description)| json!({
                "const": value,
                "description": description
            })).collect::<Vec<_>>()
        },
        "Level": {
            "type": "string",
            "oneOf": RELATION_LEVELS.into_iter().map(|(value, description)| json!({
                "const": value,
                "description": description
            })).collect::<Vec<_>>()
        }
    }))
}

fn top_output_schema() -> Value {
    let branches = top_branches();
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Successful structured result over the exact requested hour. Rows are fixed descending by whole-window product value, non-null before null. Total covers every eligible entity; Other subtracts returned rows from Total. Null means unavailable or no contributor and differs from real zero. Tool errors do not use this shape.",
        "properties": {
            "hour_start": {
                "$ref": "#/$defs/Timestamp",
                "description": "Normalized UTC-hour start; exactly the requested hour."
            },
            "hour_end": {
                "$ref": "#/$defs/Timestamp",
                "description": "Inclusive UTC-hour end; exactly hour_start + 3599999999."
            },
            "surface": {
                "$ref": "#/$defs/Surface",
                "description": "Effective semantic product surface."
            },
            "metric": {
                "$ref": "#/$defs/Metric",
                "description": "Effective semantic metric after applying the surface-specific default."
            },
            "level": {
                "$ref": "#/$defs/NullableLevel",
                "description": "Effective relation level for table/index surfaces, otherwise null."
            },
            "definition": {
                "$ref": "#/$defs/Definition",
                "description": "Exact value, unit, formula, and ranking semantics used for this result."
            },
            "intervals": {
                "type": "array",
                "minItems": 12,
                "maxItems": 60,
                "items": { "$ref": "#/$defs/Interval" },
                "description": "Exact contiguous inclusive intervals: 12 for table/index surfaces and 60 for every other surface."
            },
            "rows": {
                "type": "array",
                "maxItems": 100,
                "items": { "$ref": "#/$defs/Row" },
                "description": "At most the requested top ranked semantic entities or groups in authoritative order; an empty array is a successful no-data result."
            },
            "totals": {
                "$ref": "#/$defs/Band",
                "description": "Per-interval sum across every eligible entity. Counter total is the sum of whole-window deltas; gauge total is the maximum of the summed interval strip."
            },
            "others": {
                "$ref": "#/$defs/Band",
                "description": "Totals minus returned ranked rows. Null cells mean no omitted contributor; zero is a real difference. Gauge total is the maximum of this Other strip."
            },
            "entity_count": {
                "type": "integer",
                "minimum": 0,
                "description": "Number of ranked identities or groups before top-K truncation."
            },
            "others_count": {
                "type": "integer",
                "minimum": 0,
                "description": "entity_count minus rows.length; zero when every ranked identity or group is returned."
            },
            "top": {
                "type": "integer",
                "minimum": 0,
                "maximum": 100,
                "description": "Actual emitted row count; equals rows.length and can be below the requested top-K limit."
            },
            "out_of_order": {
                "$ref": "#/$defs/U64",
                "description": "Count of usable numeric observations whose midpoint maps to an interval earlier than that identity's already-finished interval in the ranking pass. They still update whole-window ranking. Ranking Total cells and grouped fill do not refold them; the current ungrouped winner fill can include them in an earlier row cell."
            }
        },
        "required": [
            "hour_start", "hour_end", "surface", "metric", "level", "definition",
            "intervals", "rows", "totals", "others", "entity_count", "others_count",
            "top", "out_of_order"
        ],
        "allOf": top_output_conditions(&branches),
        "$defs": top_output_definitions(&branches)
    })
}

fn top_output_conditions(branches: &[TopBranch]) -> Vec<Value> {
    branches
        .iter()
        .flat_map(|branch| {
            if branch.relation_levels {
                RELATION_LEVELS
                    .iter()
                    .map(|(level, _description)| top_output_condition(branch, Some(level)))
                    .collect::<Vec<_>>()
            } else {
                vec![top_output_condition(branch, None)]
            }
        })
        .collect()
}

fn top_output_condition(branch: &TopBranch, level: Option<&str>) -> Value {
    let mut if_properties =
        Map::from_iter([("surface".to_owned(), json!({ "const": branch.surface }))]);
    let mut required = vec!["surface"];
    if let Some(level) = level {
        if_properties.insert("level".to_owned(), json!({ "const": level }));
        required.push("level");
    }
    let (entity_definition, grouped) = entity_definition(branch.surface, level);
    let exact_cells = json!({
        "type": "array",
        "minItems": branch.intervals,
        "maxItems": branch.intervals,
        "items": { "type": ["number", "null"] }
    });
    let layout = if grouped {
        json!({ "const": null })
    } else {
        json!({ "type": "integer", "minimum": 1, "maximum": 4_294_967_295_u64 })
    };
    let members = if grouped {
        json!({ "type": "integer", "minimum": 1, "maximum": 4_294_967_295_u64 })
    } else {
        json!({ "const": null })
    };
    json!({
        "if": {
            "properties": if_properties,
            "required": required
        },
        "then": {
            "properties": {
                "metric": {
                    "type": "string",
                    "enum": branch.metrics.iter().map(|metric| metric.name).collect::<Vec<_>>()
                },
                "level": level.map_or_else(
                    || json!({ "const": null }),
                    |level| json!({ "const": level }),
                ),
                "intervals": {
                    "minItems": branch.intervals,
                    "maxItems": branch.intervals
                },
                "rows": {
                    "items": {
                        "properties": {
                            "recorded_layout": layout,
                            "entity": { "$ref": format!("#/$defs/{entity_definition}") },
                            "members": members,
                            "cells": exact_cells
                        }
                    }
                },
                "totals": { "properties": { "cells": exact_cells } },
                "others": { "properties": { "cells": exact_cells } }
            }
        }
    })
}

fn entity_definition(surface: &str, level: Option<&str>) -> (&'static str, bool) {
    match (surface, level) {
        ("postgresql_statements", None) => ("StatementEntity", false),
        ("postgresql_plans", None) => ("PlanEntity", false),
        ("postgresql_tables", Some("object")) => ("TableEntity", false),
        ("postgresql_indexes", Some("object")) => ("IndexEntity", false),
        ("processes", None) => ("ProcessEntity", true),
        ("postgresql_databases", None) => ("DatabaseEntity", false),
        ("cgroup_cpu", None) => ("CgroupCpuEntity", false),
        ("cgroup_io", None) => ("CgroupIoEntity", false),
        ("postgresql_tables" | "postgresql_indexes", Some("schema")) => {
            ("RelationSchemaEntity", true)
        }
        ("postgresql_tables" | "postgresql_indexes", Some("database")) => {
            ("RelationDatabaseEntity", true)
        }
        ("postgresql_tables" | "postgresql_indexes", Some("tablespace")) => {
            ("TablespaceEntity", true)
        }
        _ => unreachable!("registry exposes only the shipped surface and level shapes"),
    }
}

fn top_output_definitions(branches: &[TopBranch]) -> JsonObject {
    let mut definitions = scalar_definitions();
    definitions.insert("Surface".to_owned(), output_surface_schema(branches));
    definitions.insert("Metric".to_owned(), output_metric_schema());
    definitions.insert("NullableLevel".to_owned(), nullable_level_schema());
    definitions.insert("Class".to_owned(), metric_class_schema());
    definitions.insert("Unit".to_owned(), metric_unit_schema());
    definitions.insert("Ranking".to_owned(), ranking_schema());
    definitions.insert("Definition".to_owned(), value_definition_schema());
    definitions.insert("Interval".to_owned(), interval_schema());
    definitions.insert("Band".to_owned(), band_schema());
    definitions.extend(entity_schemas());
    definitions.insert("Row".to_owned(), top_row_schema());
    definitions
}

fn output_surface_schema(branches: &[TopBranch]) -> Value {
    json!({
        "type": "string",
        "oneOf": branches.iter().map(|branch| json!({
            "const": branch.surface,
            "description": branch.surface_description
        })).collect::<Vec<_>>()
    })
}

fn output_metric_schema() -> Value {
    let mut descriptions: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for metric in metric_definitions() {
        if let Some((_name, entries)) = descriptions
            .iter_mut()
            .find(|(name, _entries)| *name == metric.metric.as_str())
        {
            entries.push((metric.surface.as_str(), metric.description));
        } else {
            descriptions.push((
                metric.metric.as_str(),
                vec![(metric.surface.as_str(), metric.description)],
            ));
        }
    }
    let choices = descriptions
        .into_iter()
        .map(|(metric, entries)| {
            let description = entries
                .into_iter()
                .map(|(surface, text)| format!("{surface}: {text}"))
                .collect::<Vec<_>>()
                .join(" ");
            json!({ "const": metric, "description": description })
        })
        .collect::<Vec<_>>();
    json!({ "type": "string", "oneOf": choices })
}

fn nullable_level_schema() -> Value {
    let mut choices = vec![json!({
        "const": null,
        "description": "Non-relation surface; relation grouping does not apply."
    })];
    choices.extend(
        RELATION_LEVELS
            .into_iter()
            .map(|(value, description)| json!({ "const": value, "description": description })),
    );
    json!({ "oneOf": choices })
}

fn metric_class_schema() -> Value {
    json!({
        "type": "string",
        "oneOf": [
            {
                "const": "cumulative",
                "description": "Monotonic counter family; cells are nonnegative deltas divided by observed seconds and row totals are whole-window deltas."
            },
            {
                "const": "gauge",
                "description": "Point-reading family; cells are final assigned readings and an ungrouped row ranks by its whole-window maximum."
            }
        ]
    })
}

fn metric_unit_schema() -> Value {
    json!({
        "type": "string",
        "oneOf": [
            {
                "const": "count",
                "description": "Dimensionless event, row, scan, operation, tuple, raw-block, raw-tick, or gauge count."
            },
            {
                "const": "count_per_second",
                "description": "Count divided by observed wall-clock seconds."
            },
            { "const": "bytes", "description": "Bytes." },
            {
                "const": "bytes_per_second",
                "description": "Bytes divided by observed wall-clock seconds."
            },
            { "const": "milliseconds", "description": "Milliseconds." },
            {
                "const": "milliseconds_per_second",
                "description": "Accumulated milliseconds divided by observed wall-clock seconds."
            },
            { "const": "seconds", "description": "Seconds." },
            {
                "const": "seconds_per_second",
                "description": "Accumulated seconds divided by observed wall-clock seconds; one can equal one busy CPU core for CPU time."
            },
            { "const": "microseconds", "description": "Microseconds." },
            {
                "const": "microseconds_per_second",
                "description": "Accumulated microseconds divided by observed wall-clock seconds."
            },
            { "const": "nanoseconds", "description": "Nanoseconds." },
            {
                "const": "nanoseconds_per_second",
                "description": "Accumulated nanoseconds divided by observed wall-clock seconds."
            }
        ]
    })
}

fn ranking_schema() -> Value {
    json!({
        "type": "string",
        "oneOf": [
            {
                "const": "whole_window_delta_desc",
                "description": "Ungrouped cumulative entities ordered by nonnegative whole-window delta descending."
            },
            {
                "const": "whole_window_max_desc",
                "description": "Ungrouped gauge entities ordered by whole-window maximum descending."
            },
            {
                "const": "sum_member_window_delta_desc",
                "description": "Groups ordered by the sum of member whole-window cumulative deltas descending."
            },
            {
                "const": "sum_member_window_max_desc",
                "description": "Groups ordered by the sum of each member's whole-window gauge maximum descending."
            }
        ]
    })
}

fn value_definition_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Exact product definition selected by surface, metric, level, recorded layout, and one conversion context. Block size and clock rate are the latest usable positive recorded values at or before hour_end from the pinned source view and apply to the complete result. Missing conversion metadata preserves raw values with count/count_per_second units.",
        "properties": {
            "class": {
                "$ref": "#/$defs/Class",
                "description": "Cumulative-counter or gauge calculation family."
            },
            "cell_unit": {
                "$ref": "#/$defs/Unit",
                "description": "Exact unit of every non-null interval cell."
            },
            "total_unit": {
                "$ref": "#/$defs/Unit",
                "description": "Exact unit of every non-null row or band total; counter cells and totals intentionally use rate and base units."
            },
            "ranking": {
                "$ref": "#/$defs/Ranking",
                "description": "Fixed descending whole-window ranking formula."
            },
            "metric_description": {
                "type": "string",
                "minLength": 1,
                "description": "Short factual product definition of the selected metric."
            },
            "cell_formula": {
                "type": "string",
                "minLength": 1,
                "description": "Formula for one interval cell, including conversion and null conditions."
            },
            "total_formula": {
                "type": "string",
                "minLength": 1,
                "description": "Formula for row and band whole-hour values."
            }
        },
        "required": [
            "class", "cell_unit", "total_unit", "ranking", "metric_description",
            "cell_formula", "total_formula"
        ]
    })
}

fn interval_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "One exact inclusive display interval.",
        "properties": {
            "start": {
                "$ref": "#/$defs/Timestamp",
                "description": "Inclusive interval start in Unix microseconds."
            },
            "end": {
                "$ref": "#/$defs/Timestamp",
                "description": "Inclusive interval end in Unix microseconds."
            }
        },
        "required": ["start", "end"]
    })
}

fn band_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "One Total or Other band with the same interval shape as every row.",
        "properties": {
            "total": {
                "type": ["number", "null"],
                "description": "Finite whole-hour value in definition.total_unit, or null when no usable contributor exists; null differs from zero."
            },
            "cells": {
                "type": "array",
                "minItems": 12,
                "maxItems": 60,
                "items": { "type": ["number", "null"] },
                "description": "Finite interval values in definition.cell_unit, or null; length exactly equals intervals.length."
            }
        },
        "required": ["total", "cells"]
    })
}

fn top_row_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "One ranked semantic entity or group. cells length exactly equals intervals.length.",
        "properties": {
            "recorded_layout": {
                "type": ["integer", "null"],
                "minimum": 1,
                "maximum": 4_294_967_295_u64,
                "description": "Recorded registry type ID for an ungrouped identity; null for a semantic group that may combine layouts."
            },
            "entity": {
                "$ref": "#/$defs/Entity",
                "description": "Closed semantic key and bounded display labels for this row."
            },
            "members": {
                "type": ["integer", "null"],
                "minimum": 1,
                "maximum": 4_294_967_295_u64,
                "description": "Distinct contributing stored identities for a grouped row; null for an ungrouped row."
            },
            "total": {
                "type": ["number", "null"],
                "description": "Finite whole-hour ranking value in definition.total_unit, or null when no usable value exists; null sorts after every number."
            },
            "cells": {
                "type": "array",
                "minItems": 12,
                "maxItems": 60,
                "items": { "type": ["number", "null"] },
                "description": "Finite interval values in definition.cell_unit, or null; length exactly equals intervals.length."
            }
        },
        "required": ["recorded_layout", "entity", "members", "total", "cells"]
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "the closed entity schemas mirror the typed entity variants in one definition"
)]
fn entity_schemas() -> JsonObject {
    object(json!({
        "StatementEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded statement identity plus last non-null display labels.",
            "properties": {
                "kind": {
                    "const": "postgresql_statement",
                    "description": "PostgreSQL statement entity."
                },
                "query_id": {
                    "$ref": "#/$defs/NullableI64",
                    "description": "Recorded normalized statement Query ID; null only when the stored identity lacks a usable value."
                },
                "role_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL role OID."
                },
                "database_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL database OID."
                },
                "top_level": {
                    "type": ["boolean", "null"],
                    "description": "Top-level statement identity component; null for stored layouts that omit it."
                },
                "database_name": {
                    "$ref": "#/$defs/NullableText",
                    "description": "Last non-null recorded database-name label in the hour."
                },
                "role_name": {
                    "$ref": "#/$defs/NullableText",
                    "description": "Last non-null recorded role-name label in the hour."
                }
            },
            "required": [
                "kind", "query_id", "role_oid", "database_oid", "top_level",
                "database_name", "role_name"
            ]
        },
        "PlanEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded plan-entry identity plus last non-null display labels.",
            "properties": {
                "kind": {
                    "const": "postgresql_plan",
                    "description": "PostgreSQL execution-plan entity."
                },
                "role_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL role OID."
                },
                "database_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL database OID."
                },
                "entry_query_id": {
                    "$ref": "#/$defs/I64",
                    "description": "Recorded non-null query component of the plan-entry identity; extension meanings can vary, so this field is not asserted to be a pg_stat_statements Query ID."
                },
                "plan_id": {
                    "$ref": "#/$defs/I64",
                    "description": "Recorded non-null Plan ID."
                },
                "database_name": {
                    "$ref": "#/$defs/NullableText",
                    "description": "Last non-null recorded database-name label in the hour."
                },
                "role_name": {
                    "$ref": "#/$defs/NullableText",
                    "description": "Last non-null recorded role-name label in the hour."
                }
            },
            "required": [
                "kind", "role_oid", "database_oid", "entry_query_id", "plan_id",
                "database_name", "role_name"
            ]
        },
        "TableEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded user-table identity plus last non-null display labels.",
            "properties": {
                "kind": {
                    "const": "postgresql_table",
                    "description": "PostgreSQL user-table entity."
                },
                "database_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL database OID."
                },
                "relation_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded table OID within database_oid."
                },
                "database_name": {
                    "type": "string",
                    "description": "Last recorded database-name label in the hour."
                },
                "schema_name": {
                    "type": "string",
                    "description": "Last recorded schema-name label in the hour."
                },
                "relation_name": {
                    "type": "string",
                    "description": "Last recorded table-name label in the hour."
                }
            },
            "required": [
                "kind", "database_oid", "relation_oid", "database_name", "schema_name",
                "relation_name"
            ]
        },
        "IndexEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded user-index identity plus last non-null display labels.",
            "properties": {
                "kind": {
                    "const": "postgresql_index",
                    "description": "PostgreSQL user-index entity."
                },
                "database_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL database OID."
                },
                "index_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded index OID within database_oid."
                },
                "database_name": {
                    "type": "string",
                    "description": "Last recorded database-name label in the hour."
                },
                "schema_name": {
                    "type": "string",
                    "description": "Last recorded schema-name label in the hour."
                },
                "table_name": {
                    "type": "string",
                    "description": "Last recorded parent-table-name label in the hour."
                },
                "index_name": {
                    "type": "string",
                    "description": "Last recorded index-name label in the hour."
                }
            },
            "required": [
                "kind", "database_oid", "index_oid", "database_name", "schema_name",
                "table_name", "index_name"
            ]
        },
        "ProcessEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Process-command group fixed by each PID identity's first observed command in the hour.",
            "properties": {
                "kind": {
                    "const": "process_command",
                    "description": "Recorded process command group."
                },
                "command": {
                    "type": "string",
                    "description": "Recorded non-null command group value."
                }
            },
            "required": ["kind", "command"]
        },
        "DatabaseEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded PostgreSQL database identity.",
            "properties": {
                "kind": {
                    "const": "postgresql_database",
                    "description": "PostgreSQL database entity."
                },
                "database_oid": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded PostgreSQL database OID."
                },
                "database_name": {
                    "$ref": "#/$defs/NullableText",
                    "description": "Last non-null recorded database-name label in the hour."
                }
            },
            "required": ["kind", "database_oid", "database_name"]
        },
        "CgroupCpuEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded cgroup CPU identity.",
            "properties": {
                "kind": {
                    "const": "cgroup_cpu",
                    "description": "Cgroup CPU entity."
                },
                "path": {
                    "type": "string",
                    "description": "Exact recorded cgroup path."
                }
            },
            "required": ["kind", "path"]
        },
        "CgroupIoEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Exact recorded cgroup and block-device I/O identity.",
            "properties": {
                "kind": {
                    "const": "cgroup_io_device",
                    "description": "Cgroup block-I/O device entity."
                },
                "path": {
                    "type": "string",
                    "description": "Exact recorded cgroup path."
                },
                "major": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded Linux block-device major number."
                },
                "minor": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4_294_967_295_u64,
                    "description": "Recorded Linux block-device minor number."
                }
            },
            "required": ["kind", "path", "major", "minor"]
        },
        "RelationDatabaseEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Relation objects grouped by recorded database name.",
            "properties": {
                "kind": {
                    "const": "postgresql_relation_database",
                    "description": "Database-name relation group."
                },
                "database_name": {
                    "type": "string",
                    "description": "Recorded database-name group key."
                }
            },
            "required": ["kind", "database_name"]
        },
        "RelationSchemaEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Relation objects grouped by recorded database and schema names.",
            "properties": {
                "kind": {
                    "const": "postgresql_relation_schema",
                    "description": "Database-and-schema relation group."
                },
                "database_name": {
                    "type": "string",
                    "description": "Recorded database-name group component."
                },
                "schema_name": {
                    "type": "string",
                    "description": "Recorded schema-name group component."
                }
            },
            "required": ["kind", "database_name", "schema_name"]
        },
        "TablespaceEntity": {
            "type": "object",
            "additionalProperties": false,
            "description": "Relation objects grouped cluster-wide by the exact recorded nullable tablespace-name value.",
            "properties": {
                "kind": {
                    "const": "postgresql_tablespace",
                    "description": "Recorded tablespace-name group."
                },
                "tablespace_name": {
                    "$ref": "#/$defs/NullableText",
                    "description": "Exact group key. null is a distinct group for rows whose recorded tablespace name is absent."
                }
            },
            "required": ["kind", "tablespace_name"]
        },
        "Entity": {
            "description": "One of the closed semantic entity or relation-group shapes.",
            "oneOf": [
                { "$ref": "#/$defs/StatementEntity" },
                { "$ref": "#/$defs/PlanEntity" },
                { "$ref": "#/$defs/TableEntity" },
                { "$ref": "#/$defs/IndexEntity" },
                { "$ref": "#/$defs/ProcessEntity" },
                { "$ref": "#/$defs/DatabaseEntity" },
                { "$ref": "#/$defs/CgroupCpuEntity" },
                { "$ref": "#/$defs/CgroupIoEntity" },
                { "$ref": "#/$defs/RelationDatabaseEntity" },
                { "$ref": "#/$defs/RelationSchemaEntity" },
                { "$ref": "#/$defs/TablespaceEntity" }
            ]
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fmt::Write as _;

    use serde_json::json;
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::product::activity::{
        ActivityArgs, ActivityResult, ActivityRow, ActivitySort, normalize_activity,
    };
    use crate::product::page::Direction;
    use crate::product::top_activity::{
        Band, ConversionContext, Entity, I64String, Interval, MetricClass, MetricUnit, Ranking,
        RawQuery, Row, TopActivityResult, U64String, normalize,
    };

    const HOUR: &str = "1722506400000000";

    #[test]
    fn descriptor_set_is_exactly_the_two_approved_tools() {
        let tools = tools();
        assert_eq!(tools.len(), 2, "exactly two tools must be published");
        assert_eq!(tools[0].name, "kronika_find_top_activity");
        assert_eq!(
            tools[0].title.as_deref(),
            Some("Find top load across system and PostgreSQL")
        );
        assert_eq!(tools[1].name, "kronika_read_postgresql_activity");
        assert_eq!(
            tools[1].title.as_deref(),
            Some("Read recorded PostgreSQL backend activity")
        );
        for tool in &tools {
            let annotations = tool.annotations.as_ref().expect("tool annotations");
            assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
            assert_eq!(annotations.open_world_hint, Some(false), "{}", tool.name);
            assert_eq!(
                tool.input_schema.get("additionalProperties"),
                Some(&json!(false))
            );
            assert_eq!(
                tool.output_schema
                    .as_ref()
                    .and_then(|schema| schema.get("additionalProperties")),
                Some(&json!(false)),
                "{}",
                tool.name
            );
        }

        let descriptor_text = serde_json::to_string(&tools).expect("serialize descriptors");
        for suffix in ["heatmap", "statements", "locks", "logs", "processes"] {
            let absent = format!("kronika_read_{suffix}");
            assert!(
                !descriptor_text.contains(&absent),
                "non-published callable name {absent}"
            );
        }
        assert!(
            tools[1]
                .description
                .as_deref()
                .is_some_and(|description| description.contains("kronika_find_top_activity")),
            "Activity must point to the discovery tool"
        );
    }

    #[test]
    fn activity_schemas_match_the_frozen_v6_hashes() {
        assert_eq!(
            canonical_sha256(&activity_input_schema()),
            "79b9fbba99a849c22d2320ba3f51b9d9ae9ece33f194d697164b1e9fa59994d5"
        );
        assert_eq!(
            canonical_sha256(&activity_output_schema()),
            "51212cd257e2d38fab3ed562e7347739f619062593c7560a124ecc423befdb2d"
        );
    }

    #[test]
    fn activity_schema_and_runtime_share_sort_defaults_and_closed_arguments() {
        let input = activity_input_schema();
        let sort_values = const_values(&input["properties"]["sort"]["oneOf"]);
        let expected = [
            "pid",
            "database",
            "role",
            "query_preview",
            "query_duration_ms",
            "transaction_duration_ms",
            "application",
            "client",
            "state",
            "wait_type",
            "wait_event",
            "backend_type",
        ];
        assert_eq!(sort_values, expected);
        assert_eq!(input["properties"]["sort"]["default"], "query_duration_ms");
        assert_eq!(input["properties"]["direction"]["default"], "desc");
        assert_eq!(input["properties"]["page_size"]["default"], 200);

        for sort in expected {
            let args = ActivityArgs::from_value(json!({ "at": "0", "sort": sort }))
                .expect("schema sort parses");
            let query = normalize_activity(args).expect("schema sort normalizes");
            assert_eq!(query.sort.as_str(), sort);
        }

        let defaults = normalize_activity(
            ActivityArgs::from_value(json!({ "at": "0" })).expect("default arguments parse"),
        )
        .expect("default arguments normalize");
        assert_eq!(defaults.sort, ActivitySort::QueryDurationMs);
        assert_eq!(defaults.direction, Direction::Desc);
        assert_eq!(defaults.page.page_size, 200);

        for field in ["filter", "sort", "direction", "page_size", "cursor"] {
            let mut arguments = json!({ "at": "0" });
            arguments
                .as_object_mut()
                .expect("arguments object")
                .insert(field.to_owned(), Value::Null);
            assert!(
                ActivityArgs::from_value(arguments).is_err(),
                "explicit null must be rejected for {field}"
            );
        }
        assert!(
            ActivityArgs::from_value(json!({ "at": "0", "unknown": true })).is_err(),
            "unknown argument must be rejected"
        );
    }

    #[test]
    fn activity_output_property_names_match_typed_serialization() {
        let row = ActivityRow {
            observed_at: "1".to_owned(),
            pid: 7,
            leader_pid: None,
            datid: None,
            datname: None,
            usename: None,
            application_name: String::new(),
            client_addr: String::new(),
            backend_type: "client backend".to_owned(),
            state: Some("active".to_owned()),
            wait_event_type: None,
            wait_event: None,
            query_preview: Some("select 1".to_owned()),
            query_id: Some("-7".to_owned()),
            backend_xid_age: None,
            backend_xmin_age: None,
            backend_start: "0".to_owned(),
            xact_start: None,
            query_start: Some("1".to_owned()),
            state_change: Some("1".to_owned()),
            backend_age_ms: Some(1.0),
            query_duration_ms: Some(0.0),
            transaction_duration_ms: None,
            state_duration_ms: Some(0.0),
        };
        let result = ActivityResult {
            requested_at: "1".to_owned(),
            observed_at: Some("1".to_owned()),
            rows: vec![row.clone()],
            next_cursor: None,
        };
        let output = activity_output_schema();
        assert_required_properties(
            &output,
            &serde_json::to_value(result).expect("Activity result"),
        );
        assert_required_properties(
            &output["$defs"]["Row"],
            &serde_json::to_value(row).expect("Activity row"),
        );
        assert_local_refs_resolve(&activity_input_schema());
        assert_local_refs_resolve(&output);
    }

    #[test]
    fn top_schema_registry_matrix_matches_runtime() {
        let branches = top_branches();
        assert_eq!(branches.len(), 8);
        assert_eq!(
            branches
                .iter()
                .map(|branch| branch.metrics.len())
                .sum::<usize>(),
            37
        );

        let all_metrics = branches
            .iter()
            .flat_map(|branch| branch.metrics.iter().map(|metric| metric.name))
            .collect::<BTreeSet<_>>();
        assert_eq!(all_metrics.len(), 32);

        let mut valid_pairs = 0;
        let mut invalid_pairs = 0;
        let mut expanded = 0;
        for branch in &branches {
            let default = normalize(raw_top(branch.surface, None, None, None))
                .expect("registry default normalizes");
            assert_eq!(
                default.selection().metric().as_str(),
                branch.default_metric,
                "{} default",
                branch.surface
            );
            assert_eq!(default.top().get(), 25, "{} top default", branch.surface);

            for metric in &all_metrics {
                let accepted = normalize(raw_top(branch.surface, Some(metric), None, Some(25)));
                if branch.metrics.iter().any(|choice| choice.name == *metric) {
                    accepted.expect("registry pair must normalize");
                    valid_pairs += 1;
                } else {
                    assert!(
                        accepted.is_err(),
                        "invalid pair {}/{metric}",
                        branch.surface
                    );
                    invalid_pairs += 1;
                }
            }

            for metric in &branch.metrics {
                if branch.relation_levels {
                    for (level, _description) in RELATION_LEVELS {
                        normalize(raw_top(
                            branch.surface,
                            Some(metric.name),
                            Some(level),
                            Some(25),
                        ))
                        .expect("level-expanded selection normalizes");
                        expanded += 1;
                    }
                } else {
                    normalize(raw_top(branch.surface, Some(metric.name), None, Some(25)))
                        .expect("non-relation selection normalizes");
                    expanded += 1;
                }
            }
        }
        assert_eq!(valid_pairs, 37);
        assert_eq!(invalid_pairs, 219);
        assert_eq!(expanded, 61);
    }

    #[test]
    fn top_input_schema_is_generated_from_the_registry_matrix() {
        let input = top_input_schema();
        let branches = top_branches();
        assert_eq!(input["allOf"].as_array().map(Vec::len), Some(8));
        assert_eq!(
            input["$defs"]["Surface"]["oneOf"].as_array().map(Vec::len),
            Some(8)
        );
        assert_eq!(
            input["$defs"]["Top"]["oneOf"].as_array().map(Vec::len),
            Some(4)
        );
        assert_eq!(
            input["$defs"]["Level"]["oneOf"].as_array().map(Vec::len),
            Some(4)
        );
        for (branch, condition) in branches
            .iter()
            .zip(input["allOf"].as_array().expect("input conditions").iter())
        {
            assert_eq!(
                condition["if"]["properties"]["surface"]["const"],
                branch.surface
            );
            assert_eq!(
                condition["then"]["properties"]["metric"]["default"],
                branch.default_metric
            );
            assert_eq!(
                const_values(&condition["then"]["properties"]["metric"]["oneOf"]),
                branch
                    .metrics
                    .iter()
                    .map(|metric| metric.name)
                    .collect::<Vec<_>>()
            );
        }

        for field in ["metric", "level", "top"] {
            let mut arguments = json!({ "hour": HOUR, "surface": "cgroup_cpu" });
            arguments
                .as_object_mut()
                .expect("arguments object")
                .insert(field.to_owned(), Value::Null);
            assert!(
                serde_json::from_value::<RawQuery>(arguments).is_err(),
                "explicit null must be rejected for {field}"
            );
        }
        assert_local_refs_resolve(&input);
    }

    #[test]
    fn top_output_schema_has_fixed_registry_shapes_and_typed_names() {
        let output = top_output_schema();
        assert_eq!(output["allOf"].as_array().map(Vec::len), Some(14));
        assert_eq!(
            output["$defs"]["Metric"]["oneOf"].as_array().map(Vec::len),
            Some(32)
        );
        assert_eq!(
            output["$defs"]["Entity"]["oneOf"].as_array().map(Vec::len),
            Some(11)
        );
        assert_local_refs_resolve(&output);

        let query = normalize(raw_top(
            "postgresql_tables",
            Some("writes"),
            Some("schema"),
            Some(10),
        ))
        .expect("sample top query");
        let selection = query.selection();
        let recipe = query.recipe().expect("sample recipe");
        let definition = recipe.resolve(ConversionContext::default()).definition;
        let interval = Interval {
            start: I64String::new(query.hour().start()),
            end: I64String::new(query.hour().start()),
        };
        let cells = vec![None; recipe.intervals];
        let band = Band {
            total: None,
            cells: cells.clone(),
        };
        let result = TopActivityResult {
            hour_start: I64String::new(query.hour().start()),
            hour_end: I64String::new(query.hour().end()),
            surface: selection.surface(),
            metric: selection.metric(),
            level: selection.level(),
            definition,
            intervals: vec![interval; recipe.intervals],
            rows: vec![Row {
                recorded_layout: None,
                entity: Entity::PostgreSqlRelationSchema {
                    database_name: "app".to_owned(),
                    schema_name: "public".to_owned(),
                },
                members: Some(1),
                total: None,
                cells,
            }],
            totals: band.clone(),
            others: band,
            entity_count: 1,
            others_count: 0,
            top: 1,
            out_of_order: U64String::new(0),
        };
        assert_required_properties(&output, &serde_json::to_value(result).expect("top result"));
    }

    fn entity_schema_fixtures() -> [(&'static str, Entity); 11] {
        [
            (
                "StatementEntity",
                Entity::PostgreSqlStatement {
                    query_id: None,
                    role_oid: 1,
                    database_oid: 2,
                    top_level: None,
                    database_name: None,
                    role_name: None,
                },
            ),
            (
                "PlanEntity",
                Entity::PostgreSqlPlan {
                    role_oid: 1,
                    database_oid: 2,
                    entry_query_id: I64String::new(-3),
                    plan_id: I64String::new(4),
                    database_name: None,
                    role_name: None,
                },
            ),
            (
                "TableEntity",
                Entity::PostgreSqlTable {
                    database_oid: 1,
                    relation_oid: 2,
                    database_name: "db".to_owned(),
                    schema_name: "public".to_owned(),
                    relation_name: "t".to_owned(),
                },
            ),
            (
                "IndexEntity",
                Entity::PostgreSqlIndex {
                    database_oid: 1,
                    index_oid: 2,
                    database_name: "db".to_owned(),
                    schema_name: "public".to_owned(),
                    table_name: "t".to_owned(),
                    index_name: "i".to_owned(),
                },
            ),
            (
                "ProcessEntity",
                Entity::ProcessCommand {
                    command: "postgres".to_owned(),
                },
            ),
            (
                "DatabaseEntity",
                Entity::PostgreSqlDatabase {
                    database_oid: 1,
                    database_name: None,
                },
            ),
            (
                "CgroupCpuEntity",
                Entity::CgroupCpu {
                    path: "/system.slice".to_owned(),
                },
            ),
            (
                "CgroupIoEntity",
                Entity::CgroupIoDevice {
                    path: "/system.slice".to_owned(),
                    major: 8,
                    minor: 0,
                },
            ),
            (
                "RelationDatabaseEntity",
                Entity::PostgreSqlRelationDatabase {
                    database_name: "db".to_owned(),
                },
            ),
            (
                "RelationSchemaEntity",
                Entity::PostgreSqlRelationSchema {
                    database_name: "db".to_owned(),
                    schema_name: "public".to_owned(),
                },
            ),
            (
                "TablespaceEntity",
                Entity::PostgreSqlTablespace {
                    tablespace_name: None,
                },
            ),
        ]
    }

    #[test]
    fn top_entity_schemas_match_every_typed_entity_variant() {
        let definitions = top_output_definitions(&top_branches());
        for (definition, entity) in entity_schema_fixtures() {
            let value = serde_json::to_value(entity).expect("entity serialization");
            let schema = definitions.get(definition).expect("entity definition");
            assert_required_properties(schema, &value);
            assert_eq!(
                value["kind"], schema["properties"]["kind"]["const"],
                "{definition} kind"
            );
        }
    }

    #[test]
    fn top_definition_enum_schemas_match_typed_wire_values() {
        let definitions = top_output_definitions(&top_branches());
        assert_eq!(
            const_values(&definitions["Class"]["oneOf"]),
            ["cumulative", "gauge"]
        );
        assert_eq!(
            serde_json::to_value([MetricClass::Cumulative, MetricClass::Gauge]).expect("classes"),
            json!(["cumulative", "gauge"])
        );
        assert_eq!(
            const_values(&definitions["Unit"]["oneOf"]),
            [
                "count",
                "count_per_second",
                "bytes",
                "bytes_per_second",
                "milliseconds",
                "milliseconds_per_second",
                "seconds",
                "seconds_per_second",
                "microseconds",
                "microseconds_per_second",
                "nanoseconds",
                "nanoseconds_per_second",
            ]
        );
        assert_eq!(
            serde_json::to_value([
                MetricUnit::Count,
                MetricUnit::CountPerSecond,
                MetricUnit::Bytes,
                MetricUnit::BytesPerSecond,
                MetricUnit::Milliseconds,
                MetricUnit::MillisecondsPerSecond,
                MetricUnit::Seconds,
                MetricUnit::SecondsPerSecond,
                MetricUnit::Microseconds,
                MetricUnit::MicrosecondsPerSecond,
                MetricUnit::Nanoseconds,
                MetricUnit::NanosecondsPerSecond,
            ])
            .expect("units"),
            json!([
                "count",
                "count_per_second",
                "bytes",
                "bytes_per_second",
                "milliseconds",
                "milliseconds_per_second",
                "seconds",
                "seconds_per_second",
                "microseconds",
                "microseconds_per_second",
                "nanoseconds",
                "nanoseconds_per_second"
            ])
        );
        assert_eq!(
            serde_json::to_value([
                Ranking::WholeWindowDeltaDesc,
                Ranking::WholeWindowMaxDesc,
                Ranking::SumMemberWindowDeltaDesc,
                Ranking::SumMemberWindowMaxDesc,
            ])
            .expect("rankings"),
            json!([
                "whole_window_delta_desc",
                "whole_window_max_desc",
                "sum_member_window_delta_desc",
                "sum_member_window_max_desc"
            ])
        );
    }

    fn raw_top(
        surface: &str,
        metric: Option<&str>,
        level: Option<&str>,
        top: Option<i64>,
    ) -> RawQuery {
        RawQuery {
            hour: HOUR.to_owned(),
            surface: surface.to_owned(),
            metric: metric.map(str::to_owned),
            level: level.map(str::to_owned),
            top,
        }
    }

    fn const_values(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .expect("oneOf array")
            .iter()
            .map(|choice| choice["const"].as_str().expect("string const"))
            .collect()
    }

    fn canonical_sha256(value: &Value) -> String {
        let mut encoded =
            serde_json::to_vec(&canonical_value(value)).expect("canonical schema JSON");
        encoded.push(b'\n');
        let mut output = String::with_capacity(64);
        for byte in Sha256::digest(encoded) {
            write!(&mut output, "{byte:02x}").expect("write digest");
        }
        output
    }

    fn canonical_value(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut keys = object.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                let mut canonical = JsonObject::new();
                for key in keys {
                    canonical.insert(key.clone(), canonical_value(&object[key]));
                }
                Value::Object(canonical)
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
        }
    }

    fn assert_required_properties(schema: &Value, value: &Value) {
        let properties = schema["properties"]
            .as_object()
            .expect("schema properties")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let required = schema["required"]
            .as_array()
            .expect("schema required")
            .iter()
            .map(|name| name.as_str().expect("required name"))
            .collect::<BTreeSet<_>>();
        let actual = value
            .as_object()
            .expect("serialized object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            properties, required,
            "every typed output property is required"
        );
        assert_eq!(actual, required, "typed serialization matches schema");
    }

    fn assert_local_refs_resolve(schema: &Value) {
        fn visit(value: &Value, definitions: &JsonObject) {
            match value {
                Value::Object(object) => {
                    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                        let name = reference
                            .strip_prefix("#/$defs/")
                            .expect("only local schema refs");
                        assert!(definitions.contains_key(name), "unresolved ref {reference}");
                    }
                    for child in object.values() {
                        visit(child, definitions);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        visit(child, definitions);
                    }
                }
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
            }
        }

        let definitions = schema["$defs"].as_object().expect("schema definitions");
        visit(schema, definitions);
    }
}
