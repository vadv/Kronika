use serde_json::{Map, Value, json};

use super::{DirectSpec, activity_visibility, surface};

#[expect(
    clippy::needless_pass_by_value,
    reason = "inline JSON fixtures are consumed into independent argument maps"
)]
fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().expect("argument object").clone()
}

fn spec(section: &'static str, relation: bool) -> DirectSpec {
    DirectSpec {
        section,
        key: "test",
        search: false,
        relation,
        whole_set: false,
    }
}

#[test]
fn every_advertised_lens_resolves_and_unknown_or_malformed_values_are_rejected() {
    let cases = [
        ("pg_stat_statements", "load"),
        ("pg_stat_statements", "per_call"),
        ("pg_stat_statements", "io"),
        ("pg_stat_statements", "resources"),
        ("pg_stat_statements", "stability"),
        ("pg_store_plans", "load"),
        ("pg_store_plans", "timing"),
        ("pg_store_plans", "io"),
        ("pg_store_plans", "identity"),
        ("pg_stat_user_tables", "access"),
        ("pg_stat_user_tables", "changes"),
        ("pg_stat_user_tables", "maintenance"),
        ("pg_stat_user_tables", "size_buffers"),
        ("pg_stat_user_tables", "freeze"),
        ("pg_stat_user_indexes", "usage"),
        ("pg_stat_user_indexes", "low_activity"),
        ("pg_stat_user_indexes", "size_buffers"),
        ("pg_stat_user_indexes", "state"),
    ];
    for (section, requested) in cases {
        let direct = spec(section, section.contains("user_"));
        let resolved = surface(&arguments(json!({"lens": requested})), &direct)
            .unwrap_or_else(|failure| panic!("{section} lens {requested}: {}", failure.message));
        assert_eq!(resolved.section(), section, "{section} lens {requested}");
    }

    let statements = spec("pg_stat_statements", false);
    for args in [json!({"lens": "unknown"}), json!({"lens": null})] {
        let failure = surface(&arguments(args), &statements).expect_err("invalid lens");
        assert_eq!(failure.code, "invalid_input");
        assert_eq!(failure.parameter.as_deref(), Some("lens"));
    }
}

#[test]
fn activity_flags_require_booleans_and_preserve_both_values() {
    let visibility = activity_visibility(&arguments(json!({
        "include_idle": true,
        "include_system": true,
    })))
    .expect("valid flags");
    assert!(visibility.include_idle);
    assert!(visibility.include_system);

    let defaults = activity_visibility(&Map::new()).expect("defaults");
    assert!(!defaults.include_idle);
    assert!(!defaults.include_system);

    for args in [
        json!({"include_idle": null}),
        json!({"include_system": "true"}),
    ] {
        let failure = activity_visibility(&arguments(args)).expect_err("non-boolean flag");
        assert_eq!(failure.code, "invalid_input");
    }
}

#[test]
fn missing_snapshot_source_uses_a_direct_operational_error() {
    let failure = super::snapshot_active_position(&[]).expect_err("missing snapshot metadata");

    assert_eq!(failure.code, "snapshot_source_unavailable");
    assert_eq!(failure.message, "the snapshot has no active WAL position");
}

#[test]
fn api_source_change_is_a_direct_retryable_error() {
    let error = crate::api::ApiError::from(kronika_reader::ReaderError::from(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "source moved",
    )));
    let failure = super::api_failure(&error);

    assert_eq!(failure.code, "source_changed");
    assert_eq!(
        failure.message,
        "source changed during the read; retry the request"
    );
    assert!(failure.retryable);
}
