use super::{percent, quote};

#[test]
fn a_share_of_nothing_is_zero_rather_than_a_division_by_zero() {
    assert_eq!(percent(0, 0), 0);
    assert_eq!(percent(5, 0), 0);
}

#[test]
fn a_share_rounds_to_the_nearest_whole_percent() {
    assert_eq!(percent(1, 3), 33);
    assert_eq!(percent(2, 3), 67);
    assert_eq!(percent(1, 1), 100);
}

#[test]
fn a_huge_part_does_not_overflow_the_multiplication() {
    assert_eq!(percent(u64::MAX, u64::MAX), 100);
    assert_eq!(percent(u64::MAX / 2, u64::MAX), 50);
}

#[test]
fn a_part_larger_than_the_whole_is_capped_at_a_hundred() {
    assert_eq!(percent(3, 2), 100);
}

#[test]
fn a_plain_string_only_gains_its_quotes() {
    assert_eq!(quote("os_cpu"), "\"os_cpu\"");
}

#[test]
fn quotes_and_backslashes_are_escaped() {
    assert_eq!(quote(r#"say "hi"\"#), r#""say \"hi\"\\""#);
}

#[test]
fn a_log_line_with_newlines_stays_on_one_json_line() {
    assert_eq!(quote("two\nlines\there"), r#""two\nlines\there""#);
}

#[test]
fn a_control_character_becomes_an_escape_rather_than_a_raw_byte() {
    assert_eq!(quote("\u{1}"), "\"\\u0001\"");
}
