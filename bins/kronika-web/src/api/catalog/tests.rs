use kronika_reader::SegmentSection;
use serde_json::json;

use super::{PreparedCatalog, cursor_value, section_values};
use crate::api::CachePolicy;

#[test]
fn catalog_is_private_and_revalidated() {
    let meta = PreparedCatalog::meta();
    assert_eq!(meta.cache, CachePolicy::Revalidate);
    assert_eq!(meta.cache.header(), "private,no-cache");
    assert_eq!(meta.etag, None);
}

#[test]
fn active_cursor_components_are_decimal_strings() {
    let value = cursor_value(i64::MAX, u64::MAX);
    assert_eq!(value["segment_id"], i64::MAX.to_string());
    assert_eq!(value["wal_position"], u64::MAX.to_string());
}

#[test]
fn actual_physical_layouts_keep_logical_and_implementation_provenance() {
    let values = section_values(&[
        SegmentSection {
            type_id: 1_004_001,
            rows: 17,
            bytes: 4096,
        },
        SegmentSection {
            type_id: 4_294_967_000,
            rows: 3,
            bytes: 99,
        },
    ]);

    assert_eq!(values[0]["logical_name"], "pg_store_plans");
    assert_eq!(values[0]["physical_name"], "pg_store_plans_vadv");
    assert_eq!(values[0]["type_id"], "1004001");
    assert_eq!(values[0]["implementation"], "vadv");
    assert_eq!(values[0]["rows"], "17");
    assert_eq!(values[0]["bytes"], "4096");
    assert_eq!(values[1]["type_id"], "4294967000");
    assert_eq!(values[1]["logical_name"], json!(null));
    assert_eq!(values[1]["physical_name"], json!(null));
    assert_eq!(values[1]["implementation"], json!(null));
}
