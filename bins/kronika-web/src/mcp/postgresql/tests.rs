use serde_json::{Map, Value, json};

use super::{
    ACTIVITY_FIELDS, DATABASE_FIELDS, DirectSpec, INDEX_FIELDS, PLAN_FIELDS, STATEMENT_FIELDS,
    TABLE_FIELDS, activity_visibility, direct_order_tokens, lens, order_tokens,
};
use crate::api;
use crate::route::RelationGroup;

#[expect(
    clippy::needless_pass_by_value,
    reason = "inline JSON fixtures are consumed into independent argument maps"
)]
fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().expect("argument object").clone()
}

fn spec(
    section: &'static str,
    defaults: &'static [&'static str],
    default_order: &'static str,
    relation: bool,
) -> DirectSpec {
    DirectSpec {
        section,
        key: "test",
        defaults,
        default_order,
        search: false,
        relation,
        whole_set: false,
    }
}

#[test]
fn every_advertised_lens_resolves_and_unknown_or_malformed_values_are_rejected() {
    let cases = [
        ("pg_stat_statements", STATEMENT_FIELDS, "load"),
        ("pg_stat_statements", STATEMENT_FIELDS, "per_call"),
        ("pg_stat_statements", STATEMENT_FIELDS, "io"),
        ("pg_stat_statements", STATEMENT_FIELDS, "resources"),
        ("pg_stat_statements", STATEMENT_FIELDS, "stability"),
        ("pg_store_plans", PLAN_FIELDS, "load"),
        ("pg_store_plans", PLAN_FIELDS, "timing"),
        ("pg_store_plans", PLAN_FIELDS, "io"),
        ("pg_store_plans", PLAN_FIELDS, "identity"),
        ("pg_stat_user_tables", TABLE_FIELDS, "access"),
        ("pg_stat_user_tables", TABLE_FIELDS, "changes"),
        ("pg_stat_user_tables", TABLE_FIELDS, "maintenance"),
        ("pg_stat_user_tables", TABLE_FIELDS, "size_buffers"),
        ("pg_stat_user_tables", TABLE_FIELDS, "freeze"),
        ("pg_stat_user_indexes", INDEX_FIELDS, "usage"),
        ("pg_stat_user_indexes", INDEX_FIELDS, "low_activity"),
        ("pg_stat_user_indexes", INDEX_FIELDS, "size_buffers"),
        ("pg_stat_user_indexes", INDEX_FIELDS, "state"),
    ];
    for (section, defaults, requested) in cases {
        let direct = spec(section, defaults, "default", section.contains("user_"));
        let resolved = lens(
            &arguments(json!({"lens": requested})),
            &direct,
            section.contains("user_").then_some(RelationGroup::Object),
        )
        .unwrap_or_else(|failure| panic!("{section} lens {requested}: {}", failure.message));
        assert!(!resolved.fields.is_empty(), "{section} lens {requested}");
        assert!(resolved.fields.len() <= 32, "{section} lens {requested}");
    }

    let statements = spec(
        "pg_stat_statements",
        STATEMENT_FIELDS,
        "execution_ms_per_second",
        false,
    );
    for args in [json!({"lens": "unknown"}), json!({"lens": null})] {
        let failure = lens(&arguments(args), &statements, None).expect_err("invalid lens");
        assert_eq!(failure.code, "invalid_input");
        assert_eq!(failure.parameter.as_deref(), Some("lens"));
    }
}

#[test]
fn grouped_relation_lens_defaults_are_valid_for_the_selected_group() {
    for (section, defaults, lenses) in [
        (
            "pg_stat_user_tables",
            INDEX_FIELDS,
            ["access", "changes", "maintenance", "size_buffers", "freeze"].as_slice(),
        ),
        (
            "pg_stat_user_indexes",
            TABLE_FIELDS,
            ["usage", "low_activity", "size_buffers", "state"].as_slice(),
        ),
    ] {
        let direct = spec(section, defaults, "default", true);
        for group in [
            RelationGroup::Database,
            RelationGroup::Schema,
            RelationGroup::Tablespace,
        ] {
            for requested in lenses {
                let resolved = lens(&arguments(json!({"lens": requested})), &direct, Some(group))
                    .expect("grouped lens");
                assert!(resolved.fields.len() <= 32);
                assert!(
                    resolved
                        .fields
                        .iter()
                        .all(|field| { api::relation_field_is_available(section, group, field) }),
                    "{section} {group:?} {requested}"
                );
                assert!(api::relation_field_is_available(
                    section,
                    group,
                    resolved.default_order,
                ));
            }
        }
    }
}

#[test]
fn direct_and_relation_orders_are_strict_and_translate_only_accepted_semantics() {
    assert_eq!(
        direct_order_tokens("pg_stat_activity", "query_duration_ms"),
        Some(vec!["derived.query_duration_ms".to_owned()])
    );
    assert_eq!(
        direct_order_tokens("pg_stat_statements", "execution_ms_per_second"),
        Some(vec!["total_exec_time".to_owned(), "total_time".to_owned()])
    );
    assert_eq!(
        direct_order_tokens("pg_store_plans", "hit_pct"),
        Some(vec!["derived.hit_pct".to_owned()])
    );
    assert_eq!(direct_order_tokens("pg_stat_database", "made_up"), None);
    assert!(
        order_tokens("pg_stat_activity", "made_up", None)
            .expect_err("unknown activity order")
            .parameter
            .as_deref()
            == Some("order")
    );
    assert_eq!(
        order_tokens(
            "pg_stat_user_tables",
            "dead_pct",
            Some(RelationGroup::Object),
        )
        .expect("accepted relation order"),
        ["derived.dead_pct"]
    );
    assert_eq!(
        order_tokens(
            "pg_stat_user_tables",
            "last_vacuum_oldest",
            Some(RelationGroup::Database),
        )
        .expect("accepted aggregate order"),
        ["last_vacuum_oldest"]
    );
    assert!(
        order_tokens(
            "pg_stat_user_tables",
            "last_vacuum",
            Some(RelationGroup::Database),
        )
        .is_err()
    );

    assert!(ACTIVITY_FIELDS.contains(&"state"));
    assert!(DATABASE_FIELDS.contains(&"datname"));
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
