use super::{query, supported};
use crate::extension::ExtensionVersion;

const fn version(major: u32, minor: u32) -> ExtensionVersion {
    ExtensionVersion { major, minor }
}

#[test]
fn only_the_known_ossc_1_10_shape_is_selected() {
    assert!(!supported(version(1, 9)));
    assert!(supported(version(1, 10)));
    assert!(!supported(version(1, 11)));
    assert!(!supported(version(2, 0)));
}

#[test]
fn query_uses_the_supplied_qualified_view() {
    let sql = query("\"metrics\".\"pg_store_plans_info\"");
    assert!(sql.contains("FROM \"metrics\".\"pg_store_plans_info\""));
    assert!(sql.contains("dealloc"));
    assert!(sql.contains("stats_reset_us"));
    assert!(sql.contains("kronika:"));
}
