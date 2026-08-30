use super::{
    DecimalI64, bounded_limit, invalid_arguments, mcp_error, mcp_error_with, mcp_structured,
    storage_error,
};
use crate::api::ApiError;

#[test]
fn decimal_i64_serializes_as_a_json_string() {
    let value = serde_json::to_value(DecimalI64(9_007_199_254_740_993)).expect("serialize");
    assert_eq!(
        value,
        serde_json::Value::String("9007199254740993".to_owned())
    );
}

#[test]
fn decimal_i64_round_trips_negative_values() {
    let value = serde_json::to_value(DecimalI64(-42)).expect("serialize");
    assert_eq!(value, serde_json::Value::String("-42".to_owned()));
}

#[test]
fn bounded_limit_accepts_a_value_within_the_cap() {
    assert_eq!(bounded_limit("limit", 10, 5_000), Ok(10));
    assert_eq!(bounded_limit("limit", 5_000, 5_000), Ok(5_000));
}

#[test]
fn bounded_limit_rejects_zero_and_values_above_the_cap() {
    let over = bounded_limit("limit", 4_000_000_000, 5_000).expect_err("over cap");
    let message = over.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("limit") && message.contains("5000") && message.contains("4000000000"),
        "error must name the field, the cap and the rejected value: {message}"
    );

    let zero = bounded_limit("limit", 0, 5_000).expect_err("zero");
    let message = zero.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("limit"),
        "error must name the field: {message}"
    );
}

#[test]
fn mcp_structured_keeps_the_summary_out_of_the_structured_content_and_vice_versa() {
    let result = mcp_structured(
        serde_json::json!({ "sections": ["os_process"] }),
        "1 section",
    );
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.structured_content,
        Some(serde_json::json!({ "sections": ["os_process"] }))
    );
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0].as_text().map(|text| text.text.as_str()),
        Some("1 section")
    );
}

#[test]
fn an_error_mirrors_its_text_into_structured_content() {
    let result = mcp_error("no such sort field");
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["record"], "error");
    assert_eq!(structured["message"], "no such sort field");
    assert!(structured.get("valid_options").is_none());
}

#[test]
fn a_refusal_with_choices_carries_valid_options() {
    let result = mcp_error_with(
        "operator eq is not valid",
        vec!["gt".to_owned(), "lt".to_owned()],
    );
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["valid_options"], serde_json::json!(["gt", "lt"]));
}

#[test]
fn rejected_arguments_name_the_tool_and_its_usage() {
    let result = invalid_arguments(
        "kronika_find_events",
        "from, to, and limit are required",
        "missing field `limit`",
    );
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("kronika_find_events")
            && message.contains("Usage: from, to, and limit are required"),
        "{message}"
    );
}

#[test]
fn a_missing_section_or_column_error_names_the_listing_tool() {
    let section = storage_error(&ApiError::NoSuchSection);
    let message = section.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("kronika_get_context lists recorded sections"),
        "{message}"
    );

    let column = storage_error(&ApiError::NoSuchColumn("rss".to_owned()));
    let message = column.content[0].as_text().expect("text").text.clone();
    assert!(message.contains("kronika_get_context"), "{message}");

    let unreadable = storage_error(&ApiError::Unreadable(Box::new(std::io::Error::other(
        "broken",
    ))));
    let message = unreadable.content[0].as_text().expect("text").text.clone();
    assert!(
        !message.contains("kronika_get_context"),
        "an unreadable store has no listing to point at: {message}"
    );
}
