use hyper::StatusCode;
use kronika_reader::SegmentKind;

use super::{etag_matches, resource_meta, section_layout};
use crate::api::CachePolicy;

#[test]
fn entity_tag_matching_accepts_lists_wildcards_and_weak_validators() {
    assert!(etag_matches("\"other\", W/\"1234abcd\"", "\"1234abcd\""));
    assert!(etag_matches("*", "\"1234abcd\""));
    assert!(!etag_matches("\"1234abce\"", "\"1234abcd\""));
}

#[test]
fn every_finished_index_revalidates_even_when_a_cold_build_was_not_published() {
    let meta = resource_meta(SegmentKind::Finished, Some(0x1234_abcd)).unwrap();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::Revalidate);
    assert_eq!(meta.etag.as_deref(), Some("\"1234abcd\""));
    assert!(resource_meta(SegmentKind::Finished, None).is_err());
}

#[test]
fn active_index_has_no_validator_and_is_never_stored() {
    let meta = resource_meta(SegmentKind::Active, None).unwrap();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::NoStore);
    assert_eq!(meta.etag, None);
}

#[test]
fn health_has_one_explicit_allowlisted_series() {
    let value = section_layout("health", 0).unwrap();
    assert_eq!(value["logical_name"], "health");
    assert_eq!(value["type_id"], "0");
    assert_eq!(value["identity"].as_array().unwrap().len(), 0);
    assert_eq!(value["columns"][0]["name"], "os_health");
    assert_eq!(value["columns"][0]["class"], "gauge");
    assert_eq!(value["columns"][0]["type"], "u8");
}

#[test]
fn a_logical_section_retains_its_exact_physical_layout_provenance() {
    let value = section_layout("pg_stat_database", 1_005_004).expect("known PG18 layout");
    assert_eq!(value["logical_name"], "pg_stat_database");
    assert_eq!(value["physical_name"], "pg_stat_database");
    assert_eq!(value["type_id"], "1005004");
    assert_eq!(value["identity"][0], "datid");
    assert_eq!(value["columns"][0]["name"], "transactions_per_second");
}

#[test]
fn statement_and_plan_layouts_have_no_index_representation() {
    assert!(section_layout("pg_stat_statements", 1_002_006).is_err());
    assert!(section_layout("pg_store_plans", 1_004_001).is_err());
}
