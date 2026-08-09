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
fn health_uses_the_same_layout_and_object_record_shape_as_physical_series() {
    let value = section_layout("health", 0).unwrap();
    assert_eq!(value["logical_name"], "health");
    assert_eq!(value["type_id"], "0");
    assert_eq!(value["identity"].as_array().unwrap().len(), 0);
    assert_eq!(value["columns"][0]["name"], "health");
    assert_eq!(value["columns"][0]["class"], "gauge");
    assert_eq!(value["columns"][0]["type"], "u32");
}

#[test]
fn a_logical_section_retains_its_exact_physical_layout_provenance() {
    let value = section_layout("pg_store_plans", 1_004_001).expect("known vadv layout");
    assert_eq!(value["logical_name"], "pg_store_plans");
    assert_eq!(value["physical_name"], "pg_store_plans_vadv");
    assert_eq!(value["type_id"], "1004001");
    assert_eq!(value["implementation"], "vadv");
    assert_eq!(value["identity"][0], "userid");
    assert_eq!(value["identity"][1], "dbid");
    assert_eq!(value["identity"][2], "queryid");
    assert_eq!(value["identity"][3], "planid");
}
