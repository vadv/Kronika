use super::{ExtensionVersion, parse_version};

const fn version(major: u32, minor: u32) -> ExtensionVersion {
    ExtensionVersion { major, minor }
}

#[test]
fn a_two_part_version_reads_as_its_two_numbers() {
    assert_eq!(parse_version("1.10"), Some(version(1, 10)));
    assert_eq!(parse_version("2.0"), Some(version(2, 0)));
}

#[test]
fn a_bare_major_has_a_zero_minor() {
    assert_eq!(parse_version("2"), Some(version(2, 0)));
}

#[test]
fn a_patch_or_suffix_does_not_change_the_column_set() {
    assert_eq!(parse_version("1.10.2"), Some(version(1, 10)));
    assert_eq!(parse_version("1.10-beta1"), Some(version(1, 10)));
}

#[test]
fn something_that_is_not_a_version_is_rejected() {
    assert_eq!(parse_version(""), None);
    assert_eq!(parse_version("unknown"), None);
    assert_eq!(parse_version(".5"), None);
}

#[test]
fn versions_order_by_major_before_minor() {
    assert!(parse_version("1.12") > parse_version("1.9"));
    assert!(parse_version("2.0") > parse_version("1.12"));
}
