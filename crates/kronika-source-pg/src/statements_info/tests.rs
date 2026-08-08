use super::{query, supported};
use crate::extension::ExtensionVersion;

const fn version(major: u32, minor: u32) -> ExtensionVersion {
    ExtensionVersion { major, minor }
}

#[test]
fn info_view_starts_at_extension_1_9() {
    assert!(!supported(version(1, 8)));
    assert!(supported(version(1, 9)));
    assert!(supported(version(1, 12)));
    assert!(!supported(version(2, 0)));
}

#[test]
fn query_uses_the_supplied_qualified_view() {
    let sql = query("\"metrics\".\"pg_stat_statements_info\"");
    assert!(sql.contains("FROM \"metrics\".\"pg_stat_statements_info\""));
    assert!(sql.contains("dealloc"));
    assert!(sql.contains("stats_reset_us"));
    assert!(sql.contains("kronika:"));
}
