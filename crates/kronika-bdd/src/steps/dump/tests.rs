use super::{lines, of_kind, parse_line};

#[test]
fn a_flat_object_becomes_its_fields() {
    let line = parse_line(r#"{"kind":"section","type_id":1102001,"rows":17}"#);
    assert_eq!(line.get("kind"), Some("section"));
    assert_eq!(line.number("type_id"), Some(1_102_001));
    assert_eq!(line.number("rows"), Some(17));
}

#[test]
fn a_field_the_line_lacks_is_absent_rather_than_empty() {
    let line = parse_line(r#"{"kind":"segment"}"#);
    assert_eq!(line.get("windows"), None);
    assert_eq!(line.number("windows"), None);
    assert!(!line.holds("windows", ""));
}

#[test]
fn null_reads_as_the_word_rather_than_a_missing_field() {
    let line = parse_line(r#"{"health":null,"ts":5}"#);
    assert_eq!(line.get("health"), Some("null"));
    assert_eq!(line.number("ts"), Some(5));
}

#[test]
fn a_value_holding_a_comma_stays_whole() {
    let line = parse_line(r#"{"text":"closing because: query timeout, again","ts":5}"#);
    assert_eq!(
        line.get("text"),
        Some("closing because: query timeout, again")
    );
    assert_eq!(line.number("ts"), Some(5));
}

#[test]
fn a_value_holding_a_quote_stays_whole() {
    let line = parse_line(r#"{"text":"say \"hi\"","ts":5}"#);
    assert_eq!(line.get("text"), Some(r#"say "hi""#));
    assert_eq!(line.number("ts"), Some(5));
}

#[test]
fn escapes_come_back_as_the_characters_they_stand_for() {
    let line = parse_line(r#"{"text":"two\nlines\tapart\\here"}"#);
    assert_eq!(line.get("text"), Some("two\nlines\tapart\\here"));
}

#[test]
fn a_unicode_escape_comes_back_as_its_character() {
    let raw = "{\"text\":\"a\\u0001b\"}";
    assert_eq!(parse_line(raw).get("text"), Some("a\u{1}b"));
}

#[test]
fn a_negative_number_keeps_its_sign() {
    let line = parse_line(r#"{"delta":-42}"#);
    assert_eq!(line.number("delta"), Some(-42));
}

#[test]
fn several_lines_parse_independently() {
    let printed = "{\"kind\":\"segment\",\"windows\":3}\n{\"kind\":\"section\",\"rows\":9}\n";
    let parsed = lines(printed);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].number("windows"), Some(3));
    assert_eq!(parsed[1].number("rows"), Some(9));
}

#[test]
fn a_kind_filter_keeps_only_its_own() {
    let printed = "{\"kind\":\"segment\",\"windows\":3}\n{\"kind\":\"section\",\"rows\":9}\n";
    assert_eq!(of_kind(printed, "segment").len(), 1);
    assert_eq!(of_kind(printed, "section").len(), 1);
    assert_eq!(of_kind(printed, "point").len(), 0);
}
