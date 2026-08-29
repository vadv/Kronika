use super::{
    FIND_EVENTS_TOOL, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    FIND_PROCESSES_TOOL, GET_CONTEXT_TOOL, GET_INSTANCE_TOOL, GET_ROW_DETAIL_TOOL, OVERVIEW_TOOL,
    tools,
};

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
