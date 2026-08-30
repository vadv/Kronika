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
    assert_eq!(ranking["properties"]["top"]["default"], 25);
    assert_eq!(ranking["properties"]["top"]["maximum"], 500);
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
    assert!(encoded.contains("\"detail_locator\""), "missing locator");
}

#[test]
fn mass_event_schema_has_compact_locators_without_embedded_raw_rows() {
    let events = tools()
        .into_iter()
        .find(|tool| tool.name.as_ref() == FIND_EVENTS_TOOL)
        .expect("Events tool");
    let schema = serde_json::to_value(events.output_schema.as_ref().expect("Events output schema"))
        .expect("serialize Events schema");
    let encoded = serde_json::to_string(&schema).expect("encode Events schema");

    assert!(encoded.contains("\"detail_locator\""), "missing locator");
    assert!(encoded.contains("\"label\""), "missing bounded group label");
    for removed in ["\"text\"", "\"rows\"", "EventDataRow"] {
        assert!(
            !encoded.contains(removed),
            "Events schema retained raw mass field {removed}"
        );
    }
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
fn detail_locator_schema_and_descriptions_match_the_nested_transition() {
    let catalog = tools();
    let detail = catalog
        .iter()
        .find(|tool| tool.name.as_ref() == GET_ROW_DETAIL_TOOL)
        .expect("row detail tool");
    assert_eq!(detail.input_schema["additionalProperties"], false);
    let required = detail.input_schema["required"]
        .as_array()
        .expect("detail required fields");
    for field in [
        "section",
        "segment_id",
        "at",
        "type_id",
        "row_ordinal",
        "identity",
    ] {
        assert!(
            required.iter().any(|candidate| candidate == field),
            "{field}"
        );
    }
    assert!(
        detail.input_schema["properties"]["identity"]["type"] == "object"
            || detail.input_schema["properties"]["identity"]["$ref"].is_string()
    );
    let description = detail.description.as_deref().expect("detail description");
    assert!(description.contains("{stored_text, full_len, truncated, sha256}"));
    assert!(description.contains("never construct, guess, remove, or modify"));
    assert!(description.contains("physical hint"));
    assert!(!description.contains("plain strings"));

    for name in [
        FIND_POSTGRESQL_ACTIVITY_TOOL,
        FIND_POSTGRESQL_LOCKS_TOOL,
        FIND_POSTGRESQL_VACUUM_TOOL,
        FIND_POSTGRESQL_DATABASES_TOOL,
        FIND_POSTGRESQL_STATEMENTS_TOOL,
        FIND_POSTGRESQL_PLANS_TOOL,
        FIND_PROCESSES_TOOL,
    ] {
        let tool = catalog
            .iter()
            .find(|tool| tool.name.as_ref() == name)
            .unwrap_or_else(|| panic!("{name} tool"));
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|description| description.contains("detail_locator")),
            "{name} must advertise the nested transition"
        );
    }
}
