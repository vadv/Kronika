use serde_json::{Value, json};

use super::normalize_detail_text;

#[test]
fn every_present_detail_text_has_one_stable_shape() {
    let mut activity = json!({
        "query": "select 'привет'",
        "state": "active",
    })
    .as_object()
    .expect("activity")
    .clone();
    normalize_detail_text("pg_stat_activity", &mut activity).expect("normalize activity");
    assert_eq!(
        activity["query"],
        json!({
            "stored_text": "select 'привет'",
            "full_len": "21",
            "truncated": false,
            "sha256": null,
        })
    );
    assert_eq!(activity["state"], "active");

    let mut error = json!({
        "sample": {
            "representation": "text",
            "stored_text": "stored prefix",
            "full_len": "9000",
            "truncated": true,
            "sha256": "abc123",
        },
        "detail": null,
        "pattern": "duplicate key value violates constraint ?",
    })
    .as_object()
    .expect("error")
    .clone();
    normalize_detail_text("pg_log_errors", &mut error).expect("normalize error");
    assert_eq!(
        error["sample"],
        json!({
            "stored_text": "stored prefix",
            "full_len": "9000",
            "truncated": true,
            "sha256": "abc123",
        })
    );
    assert_eq!(error["detail"], Value::Null);
    assert!(error["sample"].get("representation").is_none());
    assert_eq!(
        error["pattern"],
        "duplicate key value violates constraint ?"
    );
}
