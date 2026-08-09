use super::{Route, RouteError, Window, parse};

#[test]
fn the_health_path_is_answered() {
    assert_eq!(parse("/api/health"), Ok((Route::Health, Window::default())));
}

#[test]
fn a_window_is_read_off_the_query() {
    let (_route, window) = parse("/api/health?from=100&to=200").expect("a window");
    assert_eq!(window.from, Some(100));
    assert_eq!(window.to, Some(200));
}

#[test]
fn one_bound_leaves_the_other_open() {
    let (_route, window) = parse("/api/health?from=100").expect("a window");
    assert_eq!(window.from, Some(100));
    assert_eq!(window.to, None);
}

#[test]
fn a_parameter_nothing_reads_is_ignored() {
    let (_route, window) = parse("/api/health?colour=green&from=5").expect("a window");
    assert_eq!(window.from, Some(5));
}

#[test]
fn a_bound_that_is_not_a_number_is_refused_by_name() {
    assert_eq!(
        parse("/api/health?from=yesterday"),
        Err(RouteError::BadParameter("from".to_owned()))
    );
}

#[test]
fn a_path_nothing_answers_is_refused() {
    assert_eq!(parse("/"), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/"), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/health/extra"), Err(RouteError::NoSuchPath));
}

#[test]
fn a_negative_bound_is_a_number_like_any_other() {
    let (_route, window) = parse("/api/health?from=-5").expect("a window");
    assert_eq!(window.from, Some(-5));
}
