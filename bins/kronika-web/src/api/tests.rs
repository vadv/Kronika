use super::within;
use crate::route::Window;

#[test]
fn an_open_window_holds_every_point() {
    assert!(within(Window::default(), i64::MIN));
    assert!(within(Window::default(), i64::MAX));
}

#[test]
fn a_point_outside_the_window_is_left_out() {
    let window = Window {
        from: Some(100),
        to: Some(200),
    };
    assert!(!within(window, 99));
    assert!(within(window, 100));
    assert!(within(window, 200));
    assert!(!within(window, 201));
}

#[test]
fn one_open_end_bounds_only_the_other() {
    let window = Window {
        from: Some(100),
        to: None,
    };
    assert!(!within(window, 99));
    assert!(within(window, i64::MAX));
}
