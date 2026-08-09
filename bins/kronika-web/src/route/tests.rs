use super::{DataRequest, Filter, Order, Route, RouteError, SegmentRequest, Window, parse};

#[test]
fn catalog_accepts_only_valid_ordered_bounds() {
    assert_eq!(
        parse("/api/catalog", Some("from=-5&to=20")),
        Ok(Route::Catalog(Window {
            from: Some(-5),
            to: Some(20),
        }))
    );
    assert_eq!(
        parse("/api/catalog", Some("from=20&to=5")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
    assert_eq!(
        parse("/api/catalog", Some("colour=green")),
        Err(RouteError::BadParameter("colour".to_owned()))
    );
}

#[test]
fn resources_name_a_segment_and_textual_section_in_the_path() {
    assert_eq!(
        parse(
            "/api/segments/1700000000000000/sections/pg_stat_statements/index",
            None,
        ),
        Ok(Route::Index(SegmentRequest {
            segment_id: 1_700_000_000_000_000,
            section: "pg_stat_statements".to_owned(),
        }))
    );
}

#[test]
fn repeated_fields_and_exact_where_parameters_keep_request_order() {
    let route = parse(
        "/api/segments/7/sections/pg_store_plans/history",
        Some("field=ts&field=planid&where.dbid=4&where.queryid=9"),
    )
    .expect("history");
    assert_eq!(
        route,
        Route::History(DataRequest {
            segment: SegmentRequest {
                segment_id: 7,
                section: "pg_store_plans".to_owned(),
            },
            fields: vec!["ts".to_owned(), "planid".to_owned()],
            filters: vec![
                Filter {
                    column: "dbid".to_owned(),
                    value: "4".to_owned(),
                },
                Filter {
                    column: "queryid".to_owned(),
                    value: "9".to_owned(),
                },
            ],
        })
    );
}

#[test]
fn rows_enforces_order_and_page_bounds() {
    let Route::Rows(request) = parse(
        "/api/segments/7/sections/os_process/rows",
        Some("order=desc&page_size=1000&cursor=7%2C0%2C0%2C50%2C99"),
    )
    .expect("rows") else {
        panic!("rows route");
    };
    assert_eq!(request.order, Order::Desc);
    assert_eq!(request.page_size, 1_000);
    assert_eq!(request.cursor.as_deref(), Some("7,0,0,50,99"));
    assert_eq!(
        parse(
            "/api/segments/7/sections/os_process/rows",
            Some("page_size=0")
        ),
        Err(RouteError::BadParameter("page_size".to_owned()))
    );
}

#[test]
fn malformed_escapes_utf8_duplicates_and_old_routes_are_refused() {
    assert_eq!(
        parse("/api/catalog", Some("from=1&from=2")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
    assert_eq!(
        parse("/api/segments/7/sections/os_cpu/history", Some("field=%ZZ")),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(parse("/api/health", None), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/top", None), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/series", None), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/rows", None), Err(RouteError::NoSuchPath));
}

#[test]
fn only_the_approved_resource_path_shape_is_recognized() {
    for path in [
        "/api/segments/7/index/os_cpu",
        "/api/segments/7/os_cpu/index",
        "/api/segments/7/section/os_cpu/index",
        "/api/segments/7/sections/os_cpu/index/",
        "/api/segments/7/sections/os_cpu",
    ] {
        assert_eq!(parse(path, None), Err(RouteError::NoSuchPath), "{path}");
    }

    assert!(matches!(
        parse("/api/segments/7/sections/os_cpu/index", None),
        Ok(Route::Index(_))
    ));
    assert!(matches!(
        parse("/api/segments/7/sections/os_cpu/history", None),
        Ok(Route::History(_))
    ));
    assert!(matches!(
        parse("/api/segments/7/sections/os_cpu/rows", None),
        Ok(Route::Rows(_))
    ));
    assert_eq!(
        parse("/api/segments/7/sections/os_cpu/index", Some("field=ts")),
        Err(RouteError::BadParameter("query".to_owned()))
    );
}

#[test]
fn a_section_is_one_strict_percent_decoded_path_component() {
    assert_eq!(
        parse(
            "/api/segments/7/sections/pg%5Fstat%5Fstatements/index",
            None,
        ),
        Ok(Route::Index(SegmentRequest {
            segment_id: 7,
            section: "pg_stat_statements".to_owned(),
        }))
    );
    assert_eq!(
        parse("/api/segments/7/sections/%FF/index", None),
        Err(RouteError::BadParameter("section".to_owned()))
    );
}
