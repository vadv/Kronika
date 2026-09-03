use std::ffi::OsString;

use super::arguments;

#[test]
fn command_line_requires_exactly_two_paths() {
    let parsed =
        arguments([OsString::from("1.zms"), OsString::from("report.html")]).expect("two paths");
    assert_eq!(parsed.0, std::path::Path::new("1.zms"));
    assert_eq!(parsed.1, std::path::Path::new("report.html"));

    assert_eq!(
        arguments(std::iter::empty()),
        Err("missing standalone ZMS input")
    );
    assert_eq!(
        arguments([OsString::from("1.zms")]),
        Err("missing HTML output")
    );
    assert_eq!(
        arguments([
            OsString::from("1.zms"),
            OsString::from("report.html"),
            OsString::from("extra"),
        ]),
        Err("expected one ZMS input and one HTML output")
    );
}
