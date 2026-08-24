use std::sync::{Arc, OnceLock};

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Value, json};

static CATALOG: OnceLock<Vec<Tool>> = OnceLock::new();

pub(super) fn all() -> &'static [Tool] {
    CATALOG.get_or_init(build)
}

pub(super) fn find(name: &str) -> Option<&'static Tool> {
    all().iter().find(|tool| tool.name == name)
}

fn build() -> Vec<Tool> {
    let mut tools = discovery_tools();
    tools.extend(surface_tools());
    tools.extend(postgresql_state_tools());
    tools.extend(postgresql_query_tools());
    tools.extend(postgresql_relation_tools());
    tools.extend(event_history_tools());
    tools.extend(expert_detail_tools());
    tools
}

fn discovery_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_get_context",
            "Kronika context",
            "Return source families, limits, surfaces, lenses, cuts, and semantic definitions.",
            input(common(map([])), &[]),
        ),
        tool(
            "kronika_list_hours",
            "Available hours",
            "List UTC hours and source families available in the requested range.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC lower bound.")),
                    ("to_us", timestamp("Inclusive UTC upper bound.")),
                    ("cursor", cursor()),
                    ("limit", integer(1, 500, 100)),
                ])),
                &[],
            ),
        ),
        tool(
            "kronika_rank_heatmap",
            "Activity Heatmap",
            "Rank Process or PostgreSQL entity activity over one bounded interval by the selected cut and grouping.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC interval start.")),
                    ("to_us", timestamp("Inclusive UTC interval end.")),
                    (
                        "surface",
                        enumeration(&[
                            "processes",
                            "statements",
                            "plans",
                            "databases",
                            "tables",
                            "indexes",
                            "cgroups",
                        ]),
                    ),
                    ("cut", short_string("Accepted metric cut for the surface.")),
                    (
                        "group",
                        enumeration(&["identity", "command", "database", "schema", "tablespace"]),
                    ),
                    ("columns", integer(1, 1_440, 12)),
                    ("top", integer(1, 500, 25)),
                ])),
                &["from_us", "to_us", "surface", "cut"],
            ),
        ),
        tool(
            "kronika_list_findings",
            "Findings",
            "List event locators and Kronika threshold crossings in a bounded interval.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC interval start.")),
                    ("to_us", timestamp("Inclusive UTC interval end.")),
                    ("surface", short_string("Optional product surface.")),
                    ("kind", enumeration(&["event", "known_bad"])),
                    ("cursor", cursor()),
                    ("limit", integer(1, 500, 100)),
                ])),
                &["from_us", "to_us"],
            ),
        ),
    ]
}

fn surface_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_get_timeline",
            "Timeline",
            "Return native timestamps for health and shared Host or PostgreSQL lanes, with sparse markers separate from series values.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC interval start.")),
                    ("to_us", timestamp("Inclusive UTC interval end.")),
                    ("lanes", string_array(16)),
                    ("cursor", cursor()),
                    ("limit", integer(1, 1_000, 200)),
                ])),
                &["from_us", "to_us"],
            ),
        ),
        tool(
            "kronika_get_host_context",
            "Host context",
            "Return Host physical capacity, state, and health values at the sample at or before one UTC time, with native units.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    (
                        "lens",
                        enumeration(&[
                            "identity",
                            "cpu",
                            "memory",
                            "storage",
                            "filesystem",
                            "network",
                            "kernel",
                            "cgroup",
                        ]),
                    ),
                    ("fields", fields()),
                    ("filters", filters()),
                    ("order", short_string("Accepted semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us", "lens"],
            ),
        ),
        tool(
            "kronika_find_processes",
            "Processes",
            "List and search Linux processes at one sample using generic, CPU, memory, disk, or bounded tree lenses.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("find", find_expression()),
                    (
                        "lens",
                        enumeration(&["generic", "cpu", "memory", "disk", "tree"]),
                    ),
                    ("order", short_string("Accepted Process semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 200)),
                ])),
                &["at_us"],
            ),
        ),
    ]
}

fn postgresql_state_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_get_postgresql_overview",
            "PostgreSQL Overview",
            "Return PostgreSQL capacity, configuration presence, service health, and cluster-wide values.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("fields", fields()),
                ])),
                &["at_us"],
            ),
        ),
        tool(
            "kronika_find_postgresql_activity",
            "PostgreSQL Activity",
            "List PostgreSQL backends at one sample, including state, wait, query and transaction durations, and optional process identifiers.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("include_idle", boolean(false)),
                    ("include_system", boolean(false)),
                    ("order", short_string("Accepted Activity semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
        tool(
            "kronika_find_postgresql_locks",
            "PostgreSQL Locks",
            "Return a bounded PostgreSQL lock graph at one sample with blocked_by edges, parents, depth, and prepared transactions.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("fields", fields()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
        tool(
            "kronika_find_postgresql_vacuum",
            "PostgreSQL Vacuum",
            "Return Vacuum episodes and phases over one bounded interval, including phase risk, progress, cycles, and movement state.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC interval start.")),
                    ("to_us", timestamp("Inclusive UTC interval end.")),
                    ("fields", fields()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["from_us", "to_us"],
            ),
        ),
    ]
}

fn postgresql_query_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_find_postgresql_statements",
            "PostgreSQL Statements",
            "List and search pg_stat_statements rows with load, per-call, I/O, resource, or stability lenses. Use find query_id:X for Query ID lookup.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("find", find_expression()),
                    (
                        "lens",
                        enumeration(&["load", "per_call", "io", "resources", "stability"]),
                    ),
                    ("order", short_string("Accepted Statement semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
        tool(
            "kronika_find_postgresql_plans",
            "PostgreSQL Plans",
            "List and search pg_store_plans rows with load, timing, I/O, or identity lenses. Use find query_id:X or plan_id:X for lookup.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("find", find_expression()),
                    ("lens", enumeration(&["load", "timing", "io", "identity"])),
                    ("order", short_string("Accepted Plan semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
        tool(
            "kronika_find_postgresql_databases",
            "PostgreSQL Databases",
            "List per-database work, failures, ages, and tones at one sample with database identity and native interval values.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("order", short_string("Accepted Database semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
    ]
}

fn postgresql_relation_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_find_postgresql_tables",
            "PostgreSQL Tables",
            "List and search PostgreSQL table access, changes, maintenance, storage, buffers, and freeze state with the relation reducer.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("find", find_expression()),
                    (
                        "lens",
                        enumeration(&[
                            "access",
                            "changes",
                            "maintenance",
                            "size_buffers",
                            "freeze",
                        ]),
                    ),
                    (
                        "group",
                        enumeration(&["object", "database", "schema", "tablespace"]),
                    ),
                    ("order", short_string("Accepted Table semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
        tool(
            "kronika_find_postgresql_indexes",
            "PostgreSQL Indexes",
            "List and search PostgreSQL index usage, low activity, storage, buffers, validity, readiness, and state severity.",
            input(
                common(map([
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("find", find_expression()),
                    (
                        "lens",
                        enumeration(&["usage", "low_activity", "size_buffers", "state"]),
                    ),
                    (
                        "group",
                        enumeration(&["object", "database", "schema", "tablespace"]),
                    ),
                    ("order", short_string("Accepted Index semantic order.")),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["at_us"],
            ),
        ),
    ]
}

fn event_history_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_find_events",
            "Event rows",
            "Search PostgreSQL and PgBouncer event rows over a bounded interval in global timestamp and physical-locator order.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC interval start.")),
                    ("to_us", timestamp("Inclusive UTC interval end.")),
                    ("sources", string_array(16)),
                    ("find", find_expression()),
                    ("order", enumeration(&["timestamp"])),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("fields", fields()),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["from_us", "to_us", "order", "direction"],
            ),
        ),
        tool(
            "kronika_get_metric_history",
            "Native metric history",
            "Return bounded native-cadence samples for selected identities and fields, preserving nulls and sample times.",
            input(
                common(map([
                    ("from_us", timestamp("Inclusive UTC interval start.")),
                    ("to_us", timestamp("Inclusive UTC interval end.")),
                    ("section", short_string("Allowlisted logical section.")),
                    ("identities", identity_array()),
                    ("fields", fields()),
                    ("sample_limit", integer(1, 10_000, 2_000)),
                ])),
                &["from_us", "to_us", "section", "fields"],
            ),
        ),
    ]
}

fn expert_detail_tools() -> Vec<Tool> {
    vec![
        tool(
            "kronika_get_snapshot",
            "Logical-section snapshot",
            "Read one allowlisted logical section at the sample at or before a UTC time.",
            input(
                common(map([
                    ("section", short_string("Allowlisted logical section.")),
                    ("at_us", timestamp("Requested sample-at-or-before time.")),
                    ("filters", filters()),
                    ("fields", fields()),
                    (
                        "order",
                        short_string("Accepted section field or semantic order."),
                    ),
                    ("direction", enumeration(&["asc", "desc"])),
                    ("cursor", cursor()),
                    ("page_size", integer(1, 500, 100)),
                ])),
                &["section", "at_us"],
            ),
        ),
        tool(
            "kronika_get_row_detail",
            "Row and text detail",
            "Read one physical row locator and an optional bounded text chunk. Timestamp, type, segment, and ordinal must match.",
            input(
                common(map([
                    ("segment_id", decimal("Decimal segment identity.")),
                    ("type_id", integer(1, u64::from(u32::MAX), 1)),
                    ("row_ordinal", decimal("Decimal physical row ordinal.")),
                    ("timestamp_us", timestamp("Row timestamp.")),
                    ("fields", fields()),
                    (
                        "text_field",
                        short_string("At most one text or blob field."),
                    ),
                    ("byte_offset", integer(0, u64::from(u32::MAX), 0)),
                    ("byte_limit", integer(1, 32 * 1_024, 16 * 1_024)),
                    ("cursor", cursor()),
                ])),
                &["segment_id", "type_id", "row_ordinal", "timestamp_us"],
            ),
        ),
    ]
}

fn tool(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: Arc<JsonObject>,
) -> Tool {
    Tool::new(name, description, input_schema)
        .with_title(title)
        .with_annotations(
            ToolAnnotations::with_title(title)
                .read_only(true)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        )
}

fn common(mut properties: JsonObject) -> JsonObject {
    properties.insert(
        "data_budget_bytes".to_owned(),
        json!({
            "type": "integer",
            "minimum": 1_024,
            "maximum": super::STRUCTURED_CONTENT_BYTES,
            "default": 32_768,
            "description": "Maximum serialized structured data bytes for this call."
        }),
    );
    properties
}

fn input(properties: JsonObject, required: &[&str]) -> Arc<JsonObject> {
    Arc::new(map([
        ("type", json!("object")),
        ("properties", Value::Object(properties)),
        ("required", json!(required)),
        ("additionalProperties", json!(false)),
    ]))
}

fn map<const N: usize>(entries: [(&str, Value); N]) -> JsonObject {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn timestamp(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[0-9]{1,20}$",
        "description": description
    })
}

fn decimal(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[0-9]+$",
        "maxLength": 32,
        "description": description
    })
}

fn short_string(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 256, "description": description})
}

fn find_expression() -> Value {
    json!({
        "type": "string",
        "maxLength": 1_024,
        "description": "The existing bounded public structured find expression."
    })
}

fn cursor() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 4_096, "description": "Opaque query-bound continuation cursor."})
}

fn fields() -> Value {
    json!({
        "type": "array",
        "items": {"type": "string", "minLength": 1, "maxLength": 128},
        "maxItems": 32,
        "uniqueItems": true
    })
}

fn string_array(max_items: usize) -> Value {
    json!({
        "type": "array",
        "items": {"type": "string", "minLength": 1, "maxLength": 128},
        "maxItems": max_items,
        "uniqueItems": true
    })
}

fn identity_array() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "maxProperties": 16,
            "additionalProperties": {"type": ["string", "integer", "boolean"]}
        },
        "maxItems": 16
    })
}

fn filters() -> Value {
    json!({
        "type": "object",
        "maxProperties": 16,
        "additionalProperties": {"type": ["string", "integer", "boolean"]}
    })
}

fn enumeration(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn integer(minimum: u64, maximum: u64, default: u64) -> Value {
    json!({
        "type": "integer",
        "minimum": minimum,
        "maximum": maximum,
        "default": default
    })
}

fn boolean(default: bool) -> Value {
    json!({"type": "boolean", "default": default})
}
