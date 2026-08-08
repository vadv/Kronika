use super::{SETTINGS_QUERY, SettingsRow, to_section};
use kronika_registry::{StrId, Ts};

fn row() -> SettingsRow {
    SettingsRow {
        ts: 5,
        name: "work_mem".to_owned(),
        setting: "4096".to_owned(),
        unit: Some("kB".to_owned()),
        source: "default".to_owned(),
        sourcefile: None,
        sourceline: None,
        pending_restart: false,
        context: "user".to_owned(),
        vartype: "integer".to_owned(),
        boot_val: Some("4096".to_owned()),
        reset_val: Some("4096".to_owned()),
    }
}

/// An interner that hands out ids in call order.
fn counting() -> impl FnMut(&[u8]) -> Result<StrId, ()> {
    let mut next = 0_u64;
    move |_bytes| {
        next += 1;
        Ok(StrId(next))
    }
}

#[test]
fn interning_follows_field_order() {
    let built = to_section(&row(), counting()).expect("the interner never fails here");
    assert_eq!(built.ts, Ts(5));
    assert_eq!(built.name, StrId(1));
    assert_eq!(built.setting, StrId(2));
    assert_eq!(built.unit, Some(StrId(3)));
    assert_eq!(built.source, StrId(4));
}

#[test]
fn an_absent_value_stays_absent_rather_than_becoming_an_id() {
    let built = to_section(&row(), counting()).expect("the interner never fails here");
    assert_eq!(built.sourcefile, None);
    assert_eq!(built.sourceline, None);
}

#[test]
fn a_value_set_from_a_file_carries_the_file_and_the_line() {
    let mut from_file = row();
    from_file.sourcefile = Some("/etc/postgresql.conf".to_owned());
    from_file.sourceline = Some(42);
    from_file.pending_restart = true;
    let built = to_section(&from_file, counting()).expect("the interner never fails here");
    assert!(built.sourcefile.is_some());
    assert_eq!(built.sourceline, Some(42));
    assert!(built.pending_restart);
}

#[test]
fn an_interner_that_fails_fails_the_row() {
    let built = to_section(&row(), |_bytes| Err("dictionary is full"));
    assert_eq!(built.map(|_row| ()), Err("dictionary is full"));
}

#[test]
fn primary_conninfo_is_excluded_before_its_value_crosses_the_connection() {
    assert!(SETTINGS_QUERY.contains("WHERE name <> 'primary_conninfo'"));
    assert!(!SETTINGS_QUERY.contains("regexp"));
    assert!(!SETTINGS_QUERY.contains("replace("));
}
