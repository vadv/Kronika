use super::{Route, RouteError, parse};

#[test]
fn native_routes_take_no_query() {
    assert_eq!(parse("/api/mcp-access", None), Ok(Route::McpAccess));
    assert_eq!(parse("/api/instance-label", None), Ok(Route::InstanceLabel));
    for path in ["/api/mcp-access", "/api/instance-label"] {
        assert_eq!(
            parse(path, Some("verbose=1")),
            Err(RouteError::BadParameter("query".to_owned()))
        );
    }
}
