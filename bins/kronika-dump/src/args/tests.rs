use super::{Want, parse};
use std::path::PathBuf;

fn args(line: &[&str]) -> Result<super::Args, String> {
    parse(line.iter().map(|word| (*word).to_owned()))
}

#[test]
fn a_directory_alone_asks_for_sizes() {
    let parsed = args(&["/data"]).expect("parse");
    assert_eq!(parsed.root, PathBuf::from("/data"));
    assert_eq!(parsed.want, Want::Sizes);
    assert!(!parsed.json);
    assert_eq!(parsed.limit, 20);
}

#[test]
fn a_section_carries_its_type_id() {
    assert_eq!(
        args(&["/data", "--section", "1107001"])
            .expect("parse")
            .want,
        Want::Section(1_107_001)
    );
}

#[test]
fn flags_may_come_before_the_directory() {
    let parsed = args(&["--json", "--index", "/data"]).expect("parse");
    assert_eq!(parsed.root, PathBuf::from("/data"));
    assert_eq!(parsed.want, Want::Index);
    assert!(parsed.json);
}

#[test]
fn the_last_of_two_wants_is_the_one_that_counts() {
    assert_eq!(
        args(&["/data", "--index", "--section", "5"])
            .expect("parse")
            .want,
        Want::Section(5)
    );
}

#[test]
fn a_limit_of_zero_means_every_row() {
    assert_eq!(args(&["/data", "--limit", "0"]).expect("parse").limit, 0);
}

#[test]
fn a_missing_directory_is_refused() {
    assert!(args(&[]).is_err());
    assert!(args(&["--json"]).is_err());
}

#[test]
fn a_flag_without_its_value_is_refused() {
    assert!(args(&["/data", "--section"]).is_err());
    assert!(args(&["/data", "--limit"]).is_err());
}

#[test]
fn a_value_that_is_not_a_number_is_refused_by_name() {
    let error = args(&["/data", "--section", "psi"]).expect_err("refused");
    assert!(
        error.contains("psi"),
        "the message names the value: {error}"
    );
}

#[test]
fn an_unknown_flag_is_refused_rather_than_ignored() {
    assert!(args(&["/data", "--colour"]).is_err());
}

#[test]
fn a_second_directory_is_refused_rather_than_silently_dropped() {
    assert!(args(&["/one", "/two"]).is_err());
}
