use std::io::Write as _;

use super::{MAX_LINE_BYTES, Position, Record, Tail};

/// Every line opens its own record.
fn never(_open: &[String], _line: &str) -> bool {
    false
}

/// A tab continues the line before it, as `PgBouncer` and wrapped `stderr`
/// messages are written.
fn tabbed(_open: &[String], line: &str) -> bool {
    line.starts_with('\t')
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).expect("write the log");
}

fn append(path: &std::path::Path, text: &str) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open the log");
    file.write_all(text.as_bytes()).expect("append to the log");
}

fn texts(records: &[Record]) -> Vec<String> {
    records.iter().map(Record::joined).collect()
}

#[test]
fn reads_only_what_arrived_since_the_last_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "first\n");
    let mut tail = Tail::new(path.clone(), Position::default());

    assert_eq!(texts(&tail.read(never).expect("first read")), ["first"]);
    append(&path, "second\n");
    assert_eq!(texts(&tail.read(never).expect("second read")), ["second"]);
    assert!(tail.read(never).expect("third read").is_empty());
}

#[test]
fn a_line_without_its_newline_waits_for_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "half");
    let mut tail = Tail::new(path.clone(), Position::default());

    assert!(tail.read(never).expect("partial read").is_empty());
    assert_eq!(tail.position().offset, 0);
    append(&path, " and half\n");
    assert_eq!(
        texts(&tail.read(never).expect("completed read")),
        ["half and half"]
    );
}

#[test]
fn a_truncated_file_is_read_from_its_start_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "a long line that is later cut away\n");
    let mut tail = Tail::new(path.clone(), Position::default());
    tail.read(never).expect("first read");

    write(&path, "after\n");

    assert_eq!(texts(&tail.read(never).expect("read after truncate")), [
        "after"
    ]);
}

#[test]
fn a_rotated_file_is_read_from_its_start_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "before rotation\n");
    let mut tail = Tail::new(path.clone(), Position::default());
    tail.read(never).expect("first read");

    std::fs::rename(&path, dir.path().join("postgresql.log.1")).expect("rotate");
    write(&path, "after rotation\n");

    assert_eq!(texts(&tail.read(never).expect("read after rotation")), [
        "after rotation"
    ]);
}

#[test]
fn an_over_long_line_is_cut_and_the_next_one_survives() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    let long = "x".repeat(MAX_LINE_BYTES + 4096);
    write(&path, &format!("{long}\nshort\n"));
    let mut tail = Tail::new(path, Position::default());

    let records = tail.read(never).expect("read");

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].first().len(), MAX_LINE_BYTES);
    assert_eq!(records[1].first(), "short");
}

#[test]
fn a_line_that_is_not_utf8_is_dropped_and_the_next_one_survives() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    std::fs::write(&path, b"bad \xff\xfe line\ngood line\n").expect("write");
    let mut tail = Tail::new(path, Position::default());

    assert_eq!(texts(&tail.read(never).expect("read")), ["good line"]);
}

#[test]
fn continuations_join_the_line_they_belong_to() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "opening\n\tcontinued\n\tagain\nnext\n");
    let mut tail = Tail::new(path, Position::default());

    assert_eq!(texts(&tail.read(tabbed).expect("read")), [
        "opening\n\tcontinued\n\tagain",
        "next"
    ]);
}

#[test]
fn a_continuation_written_after_the_read_still_reaches_its_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "opening\n");
    let mut tail = Tail::new(path.clone(), Position::default());

    // The record is still open, so nothing is handed over yet.
    assert!(tail.read(tabbed).expect("first read").is_empty());
    append(&path, "\tcontinued\n");

    assert_eq!(texts(&tail.read(tabbed).expect("second read")), [
        "opening\n\tcontinued"
    ]);
}

#[test]
fn a_record_nothing_followed_is_handed_over_on_the_next_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "alone\n");
    let mut tail = Tail::new(path, Position::default());

    assert!(tail.read(tabbed).expect("first read").is_empty());
    assert_eq!(texts(&tail.read(tabbed).expect("second read")), ["alone"]);
}

#[test]
fn a_remembered_offset_resumes_where_the_previous_process_stopped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "already read\n");
    let mut first = Tail::new(path.clone(), Position::default());
    first.read(never).expect("first read");
    append(&path, "written while stopped\n");

    let mut second = Tail::new(path, first.position());

    assert_eq!(texts(&second.read(never).expect("resumed read")), [
        "written while stopped"
    ]);
}
