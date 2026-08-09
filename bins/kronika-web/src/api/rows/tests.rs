use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_meminfo::OsMeminfo;
use kronika_registry::{Cell, Row, Section};

use super::within;
use crate::route::Window;

fn diskstats(ts: i64) -> Row {
    let mut cells = vec![Cell::Ts(ts)];
    cells.resize(OsDiskstats::CONTRACT.columns.len(), Cell::Null);
    Row::new(&OsDiskstats::CONTRACT, cells)
}

#[test]
fn an_open_window_holds_every_row() {
    assert!(within(Window::default(), &diskstats(1_000)));
}

#[test]
fn a_row_outside_the_window_is_left_out() {
    let window = Window {
        from: Some(100),
        to: Some(200),
    };
    assert!(!within(window, &diskstats(99)));
    assert!(within(window, &diskstats(100)));
    assert!(within(window, &diskstats(200)));
    assert!(!within(window, &diskstats(201)));
}

#[test]
fn a_row_without_a_timestamp_is_answered_whole() {
    let row = Row::new(&OsMeminfo::CONTRACT, Vec::new());
    let window = Window {
        from: Some(100),
        to: Some(200),
    };
    assert!(
        within(window, &row),
        "a row with no time of its own has none to compare"
    );
}
