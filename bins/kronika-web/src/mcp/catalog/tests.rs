use super::{
    FIND_EVENTS_TOOL, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    FIND_PROCESSES_TOOL, GET_CONTEXT_TOOL, GET_INSTANCE_TOOL, GET_ROW_DETAIL_TOOL, OVERVIEW_TOOL,
    tools,
};

const FINDER_TOOLS: [&str; 9] = [
    FIND_POSTGRESQL_TABLES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL,
    FIND_POSTGRESQL_ACTIVITY_TOOL,
    FIND_POSTGRESQL_LOCKS_TOOL,
    FIND_POSTGRESQL_VACUUM_TOOL,
    FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL,
    FIND_POSTGRESQL_PLANS_TOOL,
    FIND_PROCESSES_TOOL,
];

const FORBIDDEN_PUBLIC_FIELDS: [&str; 5] = [
    "detail_locator",
    "type_id",
    "segment_id",
    "row_ordinal",
    "row_key",
];

fn assert_no_internal_coordinates(label: &str, value: &serde_json::Value) {
    let encoded = serde_json::to_string(value).expect("encode public schema");
    for field in FORBIDDEN_PUBLIC_FIELDS {
        assert!(
            !encoded.contains(field),
            "{label} exposes internal field {field}: {encoded}"
        );
    }
    assert!(
        !encoded.contains("DetailLocator"),
        "{label} retains the internal DetailLocator definition: {encoded}"
    );
}

fn find_property<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    match value {
        serde_json::Value::Object(object) => object
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .and_then(|properties| properties.get(name))
            .or_else(|| object.values().find_map(|child| find_property(child, name))),
        serde_json::Value::Array(values) => {
            values.iter().find_map(|child| find_property(child, name))
        }
        _ => None,
    }
}

#[test]
fn catalog_has_exactly_fourteen_tools() {
    let catalog = tools();
    let names: Vec<&str> = catalog.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        vec![
            OVERVIEW_TOOL,
            GET_CONTEXT_TOOL,
            GET_INSTANCE_TOOL,
            FIND_POSTGRESQL_TABLES_TOOL,
            FIND_POSTGRESQL_INDEXES_TOOL,
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            FIND_POSTGRESQL_LOCKS_TOOL,
            FIND_POSTGRESQL_VACUUM_TOOL,
            FIND_POSTGRESQL_DATABASES_TOOL,
            FIND_POSTGRESQL_STATEMENTS_TOOL,
            FIND_POSTGRESQL_PLANS_TOOL,
            FIND_PROCESSES_TOOL,
            GET_ROW_DETAIL_TOOL,
            FIND_EVENTS_TOOL,
        ]
    );
}

#[test]
fn public_schemas_omit_uint_format_and_keep_nonnegative_integer_bounds() {
    let catalog = tools();
    for tool in &catalog {
        let input = serde_json::Value::Object(tool.input_schema.as_ref().clone());
        assert!(
            !serde_json::to_string(&input)
                .expect("encode input schema")
                .contains("\"format\":\"uint\"")
        );
        if let Some(output) = tool.output_schema.as_ref() {
            assert!(
                !serde_json::to_string(output)
                    .expect("encode output schema")
                    .contains("\"format\":\"uint\"")
            );
        }
    }

    for (tool_name, fields) in [
        (OVERVIEW_TOOL, &["top"][..]),
        (
            FIND_EVENTS_TOOL,
            &["runs", "completes", "timed", "requested", "waiters"][..],
        ),
    ] {
        let schema = catalog
            .iter()
            .find(|tool| tool.name.as_ref() == tool_name)
            .and_then(|tool| tool.output_schema.as_ref())
            .unwrap_or_else(|| panic!("{tool_name} output schema"));
        let schema = serde_json::Value::Object(schema.as_ref().clone());
        for field in fields {
            let property = find_property(&schema, field)
                .unwrap_or_else(|| panic!("{tool_name}.{field} schema"));
            assert_eq!(property["type"], "integer", "{tool_name}.{field}");
            assert_eq!(property["minimum"], 0, "{tool_name}.{field}");
            assert!(property.get("format").is_none(), "{tool_name}.{field}");
        }
    }
}

#[test]
fn overview_schema_exposes_the_ordered_batch_and_nested_top_cap() {
    let overview = tools()
        .into_iter()
        .find(|tool| tool.name.as_ref() == OVERVIEW_TOOL)
        .expect("overview tool");
    let required = overview.input_schema["required"]
        .as_array()
        .expect("required array")
        .iter()
        .map(|value| value.as_str().expect("string").to_owned())
        .collect::<std::collections::HashSet<_>>();
    for field in ["from", "to", "rankings"] {
        assert!(required.contains(field), "missing required field: {field}");
    }
    assert_eq!(
        overview.input_schema["properties"]["rankings"]["items"]["$ref"],
        "#/$defs/OverviewRankingInput"
    );
    let ranking = &overview.input_schema["$defs"]["OverviewRankingInput"];
    let ranking_required = ranking["required"]
        .as_array()
        .expect("ranking required array");
    assert!(ranking_required.contains(&serde_json::json!("section")));
    assert!(ranking_required.contains(&serde_json::json!("fields")));
    assert!(!ranking_required.contains(&serde_json::json!("top")));
    let fields = &ranking["properties"]["fields"];
    assert_eq!(fields["minItems"], 1);
    assert_eq!(fields["maxItems"], 4);
    assert!(fields.get("uniqueItems").is_none());
    assert_eq!(ranking["properties"]["top"]["default"], 25);
    assert_eq!(ranking["properties"]["top"]["maximum"], 500);
    let description = overview.description.as_ref().expect("description");
    assert!(description.contains("separate result"));
    assert!(description.contains("request order"));
    for removed in ["distinct numeric", "compatible unit", "Fields are summed"] {
        assert!(!description.contains(removed), "{removed}");
    }
}

#[test]
fn overview_output_schema_is_rankings_only() {
    let overview = tools()
        .into_iter()
        .find(|tool| tool.name.as_ref() == OVERVIEW_TOOL)
        .expect("overview tool");
    let schema = serde_json::to_value(
        overview
            .output_schema
            .as_ref()
            .expect("overview output schema"),
    )
    .expect("serialize output schema");
    let encoded = serde_json::to_string(&schema).expect("encode output schema");
    assert!(
        !encoded.contains("\"grid\""),
        "MCP schema exposed HTTP grid"
    );
    assert!(
        !encoded.contains("\"cells\""),
        "MCP schema exposed HTTP entity cells"
    );
    assert!(encoded.contains("\"results\""), "missing ordered results");
    for removed in [
        "as_of",
        "recorded_from",
        "recorded_to",
        "nearest_row_before",
        "nearest_row_after",
    ] {
        assert!(!encoded.contains(&format!("\"{removed}\"")), "{removed}");
    }
    assert!(encoded.contains("\"detail_ref\""), "missing detail_ref");
    let fields = &schema["$defs"]["NormalizedRanking"]["properties"]["fields"];
    assert_eq!(fields["minItems"], 1);
    assert_eq!(fields["maxItems"], 1);
    assert!(
        schema["$defs"]["HeatmapItemResult"]["properties"]
            .get("unit")
            .is_some(),
        "missing result unit",
    );
    let entity = &schema["$defs"]["HeatmapEntity"];
    assert_eq!(entity["properties"]["detail_ref"]["type"], "string");
    assert!(entity["properties"].get("identity").is_some());
    assert_no_internal_coordinates("overview output", &schema);
}

#[test]
fn mass_event_schema_has_opaque_detail_refs_without_embedded_raw_rows() {
    let events = tools()
        .into_iter()
        .find(|tool| tool.name.as_ref() == FIND_EVENTS_TOOL)
        .expect("Events tool");
    let schema = serde_json::to_value(events.output_schema.as_ref().expect("Events output schema"))
        .expect("serialize Events schema");
    let encoded = serde_json::to_string(&schema).expect("encode Events schema");
    let branch_types = schema["oneOf"]
        .as_array()
        .expect("Events output alternatives")
        .iter()
        .map(|branch| branch["type"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(branch_types, vec![Some("object"), Some("object")]);
    assert!(encoded.contains("\"detail_ref\""), "missing detail_ref");
    assert!(encoded.contains("\"label\""), "missing bounded group label");
    for definition in ["EventGroup", "EventOccurrence"] {
        assert_eq!(
            schema["$defs"][definition]["properties"]["detail_ref"]["type"], "string",
            "{definition}.detail_ref"
        );
    }
    for removed in ["\"text\"", "\"rows\"", "EventDataRow"] {
        assert!(
            !encoded.contains(removed),
            "Events schema retained raw mass field {removed}"
        );
    }
    assert_no_internal_coordinates("Events output", &schema);
}

#[test]
fn time_input_schemas_and_runtime_accept_decimal_string_outputs() {
    let catalog = tools();
    let time_fields: [(&str, &[&str]); 11] = [
        (OVERVIEW_TOOL, &["from", "to"]),
        (FIND_EVENTS_TOOL, &["from", "to"]),
        (FIND_POSTGRESQL_TABLES_TOOL, &["at"]),
        (FIND_POSTGRESQL_INDEXES_TOOL, &["at"]),
        (FIND_POSTGRESQL_ACTIVITY_TOOL, &["at"]),
        (FIND_POSTGRESQL_LOCKS_TOOL, &["at"]),
        (FIND_POSTGRESQL_VACUUM_TOOL, &["at"]),
        (FIND_POSTGRESQL_DATABASES_TOOL, &["at"]),
        (FIND_POSTGRESQL_STATEMENTS_TOOL, &["at"]),
        (FIND_POSTGRESQL_PLANS_TOOL, &["at"]),
        (FIND_PROCESSES_TOOL, &["at"]),
    ];
    for (name, fields) in time_fields {
        let tool = catalog
            .iter()
            .find(|tool| tool.name.as_ref() == name)
            .unwrap_or_else(|| panic!("{name} tool"));
        let definition = &tool.input_schema["$defs"]["TimeSpecInput"];
        let description = definition["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} TimeSpec description"));
        assert!(
            description.contains("passed unchanged"),
            "{name}: {description}"
        );
        let schema_text = serde_json::to_string(definition).expect("encode TimeSpec schema");
        assert!(schema_text.contains("\"integer\""), "{name}: {schema_text}");
        assert!(
            schema_text.contains("decimal-string"),
            "{name}: {schema_text}"
        );
        for field in fields {
            let field_schema = serde_json::to_string(&tool.input_schema["properties"][*field])
                .expect("encode time field schema");
            assert!(
                field_schema.contains("#/$defs/TimeSpecInput"),
                "{name}.{field}: {field_schema}"
            );
        }
    }

    for timestamp in [i64::MIN, i64::MAX] {
        let input: crate::mcp::time::TimeSpecInput =
            serde_json::from_value(serde_json::json!(timestamp.to_string()))
                .expect("schema-advertised decimal string");
        assert_eq!(input.resolve(0), Ok(timestamp));
    }
}

#[test]
fn every_limit_or_top_field_documents_its_runtime_cap_in_the_json_schema() {
    // The schema exposes the cap before dispatch.
    let capped = [
        (FIND_POSTGRESQL_TABLES_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_INDEXES_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_ACTIVITY_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_LOCKS_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_VACUUM_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_DATABASES_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_STATEMENTS_TOOL, "limit", 5_000),
        (FIND_POSTGRESQL_PLANS_TOOL, "limit", 5_000),
        (FIND_PROCESSES_TOOL, "limit", 5_000),
        (FIND_EVENTS_TOOL, "limit", 5_000),
    ];
    let catalog = tools();
    for (tool_name, field, max) in capped {
        let tool = catalog
            .iter()
            .find(|tool| tool.name.as_ref() == tool_name)
            .unwrap_or_else(|| panic!("{tool_name} tool"));
        let maximum = &tool.input_schema["properties"][field]["maximum"];
        assert_eq!(
            maximum,
            &serde_json::json!(max),
            "{tool_name}.{field} maximum differs: expected {max}, got {maximum:?}"
        );
    }
}

#[test]
fn no_tool_description_uses_banned_reasoning_words() {
    let banned = [
        "confidence",
        "anomaly",
        "diagnos",
        "causal",
        "recommend",
        "root cause",
    ];
    for tool in tools() {
        let description = tool
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        for word in banned {
            assert!(
                !description.contains(word),
                "{}: description contains banned word '{word}'",
                tool.name
            );
        }
    }
}

#[test]
fn context_and_instance_schemas_stay_closed_objects() {
    for name in [GET_CONTEXT_TOOL, GET_INSTANCE_TOOL] {
        let tool = tools()
            .into_iter()
            .find(|tool| tool.name.as_ref() == name)
            .expect("catalog tool");
        assert_eq!(tool.input_schema["type"], "object", "{name}");
        assert_eq!(tool.input_schema["additionalProperties"], false, "{name}");
    }
}

#[test]
fn instance_schema_exposes_scope_default_and_complete_typed_result() {
    let tool = tools()
        .into_iter()
        .find(|tool| tool.name.as_ref() == GET_INSTANCE_TOOL)
        .expect("instance tool");
    assert_eq!(
        tool.input_schema["properties"]["settings"]["default"],
        "non_default"
    );
    assert_eq!(
        tool.input_schema["properties"]["settings"]["$ref"],
        "#/$defs/SettingsScopeInput"
    );
    let settings_scopes = tool.input_schema["$defs"]["SettingsScopeInput"]["oneOf"]
        .as_array()
        .expect("settings scope choices")
        .iter()
        .map(|choice| choice["const"].as_str().expect("settings scope const"))
        .collect::<Vec<_>>();
    assert_eq!(settings_scopes, ["non_default", "all"]);
    let output = tool.output_schema.as_ref().expect("instance output schema");
    assert_eq!(output["additionalProperties"], false);
    let required = output["required"].as_array().expect("required outputs");
    for field in [
        "host",
        "host_as_of",
        "postgresql_settings",
        "settings_as_of",
        "settings_scope",
        "settings_returned_count",
        "settings_defaults_omitted",
        "settings_request_all",
    ] {
        assert!(
            required.iter().any(|candidate| candidate == field),
            "missing required instance output {field}"
        );
    }
    assert_eq!(
        output["properties"]["settings_request_all"]["$ref"],
        "#/$defs/AllSettingsRequest"
    );
    let request_all = &output["$defs"]["AllSettingsRequest"];
    assert_eq!(request_all["additionalProperties"], false);
    assert_eq!(request_all["required"], serde_json::json!(["settings"]));
    assert_eq!(
        request_all["properties"]["settings"]["$ref"],
        "#/$defs/AllSettingsScope"
    );
    assert_eq!(
        output["$defs"]["AllSettingsScope"]["enum"],
        serde_json::json!(["all"])
    );
    assert!(output["properties"].get("settings_has_more").is_none());
}

#[test]
fn finder_schemas_expose_optional_time_and_the_exact_runtime_envelope() {
    let catalog = tools();
    for name in FINDER_TOOLS {
        let tool = catalog
            .iter()
            .find(|tool| tool.name.as_ref() == name)
            .unwrap_or_else(|| panic!("{name} tool"));
        let required = tool.input_schema["required"]
            .as_array()
            .expect("input required array");
        assert!(
            tool.input_schema["properties"]["at"].is_object(),
            "{name}.at"
        );
        assert!(
            !required.iter().any(|field| field == "at"),
            "{name}.at must remain optional"
        );
        assert_eq!(tool.input_schema["additionalProperties"], false, "{name}");
        assert_eq!(
            tool.input_schema["properties"]["filters"]["maxItems"], 8,
            "{name}.filters"
        );

        let output = tool.output_schema.as_ref().expect("finder output schema");
        assert_eq!(output["additionalProperties"], false, "{name}");
        let output_required = output["required"]
            .as_array()
            .expect("output required array");
        for field in ["rows", "truncated"] {
            assert!(
                output_required.iter().any(|required| required == field),
                "{name} output omits required {field}"
            );
        }
        assert_eq!(output_required.len(), 2, "{name}");
        assert_eq!(output["properties"]["rows"]["type"], "array", "{name}");
        assert_eq!(
            output["properties"]["truncated"]["type"], "boolean",
            "{name}"
        );
        assert!(output["properties"].get("as_of").is_none(), "{name}");
        let description = tool.description.as_deref().expect("description");
        for removed in ["as_of", "cadence", "freshness", "nearest"] {
            assert!(
                !description.to_ascii_lowercase().contains(removed),
                "{name} advertises removed {removed} metadata"
            );
        }
        for obsolete in ["has_more", "next_from", "next_cursor", "cursor"] {
            assert!(
                output["properties"].get(obsolete).is_none(),
                "{name} advertises obsolete {obsolete}"
            );
        }
    }
}

#[test]
fn row_detail_accepts_only_one_opaque_string() {
    let catalog = tools();
    let detail = catalog
        .iter()
        .find(|tool| tool.name.as_ref() == GET_ROW_DETAIL_TOOL)
        .expect("row detail tool");
    assert_eq!(detail.input_schema["type"], "object");
    assert_eq!(detail.input_schema["additionalProperties"], false);
    assert_eq!(
        detail.input_schema["required"],
        serde_json::json!(["detail_ref"])
    );
    let properties = detail.input_schema["properties"]
        .as_object()
        .expect("detail input properties");
    assert_eq!(properties.len(), 1);
    assert_eq!(properties["detail_ref"]["type"], "string");
    assert_eq!(properties["detail_ref"]["minLength"], 1);
    assert_eq!(
        properties["detail_ref"]["maxLength"],
        crate::api::row_key::DETAIL_REF_MAX_ENCODED_BYTES
    );
    let description = detail.description.as_deref().expect("detail description");
    assert!(description.contains("{stored_text, full_len, truncated, sha256}"));
    assert!(description.contains("Pass a reference"));
    assert_no_internal_coordinates(
        "row detail input",
        &serde_json::Value::Object(detail.input_schema.as_ref().clone()),
    );
}

#[test]
fn tools_list_exposes_no_internal_coordinate_names() {
    for tool in tools() {
        let input = serde_json::Value::Object(tool.input_schema.as_ref().clone());
        assert_no_internal_coordinates(&format!("{} input", tool.name), &input);
        if let Some(output) = tool.output_schema.as_ref() {
            let output = serde_json::Value::Object(output.as_ref().clone());
            assert_no_internal_coordinates(&format!("{} output", tool.name), &output);
        }

        let description = tool.description.as_deref().unwrap_or_default();
        for field in FORBIDDEN_PUBLIC_FIELDS {
            assert!(
                !description.contains(field),
                "{} description exposes internal field {field}: {description}",
                tool.name
            );
        }
    }
}
