use super::{continues, split};

#[test]
fn quoted_fields_keep_their_commas_and_doubled_quotes() {
    let fields = split(r#"a,"b,c","say ""hi""",,d"#);

    assert_eq!(fields, ["a", "b,c", r#"say "hi""#, "", "d"]);
}

#[test]
fn a_record_continues_while_a_quoted_field_is_open() {
    let open = vec![r#"2026-08-07 12:34:56.789 MSK,"alice","shop",1,"#.to_owned()];
    assert!(!continues(&open, "next line"));

    let open = vec![r#"...,"select 1"#.to_owned()];
    assert!(continues(&open, "from t"));
}
