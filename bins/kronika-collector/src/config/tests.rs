use super::{RetentionConfig, parse_retention, validate_journal_max_bytes, validate_retention};
use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_LEN};

#[test]
fn a_bare_byte_budget_is_a_fixed_target() {
    assert_eq!(
        parse_retention("1073741824").expect("a byte budget"),
        RetentionConfig::Fixed(1_073_741_824)
    );
}

#[test]
fn auto_without_a_suffix_uses_the_default_percentage() {
    assert_eq!(
        parse_retention("auto").expect("auto"),
        RetentionConfig::Auto(80)
    );
    assert_eq!(
        parse_retention(" auto:55 ").expect("auto with a percentage"),
        RetentionConfig::Auto(55)
    );
}

#[test]
fn an_out_of_range_or_malformed_target_is_rejected() {
    assert!(parse_retention("").is_err());
    assert!(parse_retention("auto:0").is_err());
    assert!(parse_retention("auto:100").is_err());
    assert!(parse_retention("auto-80").is_err());
    assert!(parse_retention("plenty").is_err());
}

#[test]
fn a_fixed_budget_below_two_segments_cannot_converge() {
    let segment = 64 * 1024 * 1024;
    assert!(validate_retention(RetentionConfig::Fixed(2 * segment), segment).is_ok());
    assert!(validate_retention(RetentionConfig::Fixed(2 * segment - 1), segment).is_err());
    // `auto` targets a live partition fraction and has no such floor.
    assert!(validate_retention(RetentionConfig::Auto(1), segment).is_ok());
}

#[test]
fn the_journal_cap_must_fit_the_format() {
    assert!(validate_journal_max_bytes(JOURNAL_HEADER_LEN as u64).is_ok());
    assert!(validate_journal_max_bytes(MAX_JOURNAL_LEN as u64).is_ok());
    assert!(validate_journal_max_bytes(JOURNAL_HEADER_LEN as u64 - 1).is_err());
    assert!(validate_journal_max_bytes(MAX_JOURNAL_LEN as u64 + 1).is_err());
}
