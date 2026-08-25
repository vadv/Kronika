use super::{
    FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL, FIND_POSTGRESQL_INDEXES_TOOL,
    FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    FIND_PROCESSES_TOOL, GET_CONTEXT_TOOL, GET_ROW_DETAIL_TOOL, OVERVIEW_TOOL, tools,
};

#[test]
fn the_catalog_has_exactly_these_ten_tools_so_far() {
    let catalog = tools();
    let names: Vec<&str> = catalog.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        vec![
            OVERVIEW_TOOL,
            GET_CONTEXT_TOOL,
            FIND_POSTGRESQL_TABLES_TOOL,
            FIND_POSTGRESQL_INDEXES_TOOL,
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            FIND_POSTGRESQL_LOCKS_TOOL,
            FIND_POSTGRESQL_VACUUM_TOOL,
            FIND_POSTGRESQL_DATABASES_TOOL,
            FIND_PROCESSES_TOOL,
            GET_ROW_DETAIL_TOOL,
        ]
    );
}

#[test]
fn overview_schema_requires_section_fields_from_to_top() {
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
    for field in ["section", "fields", "from", "to", "top"] {
        assert!(required.contains(field), "missing required field: {field}");
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
