use super::{
    ActiveCursor, DEFAULT_SNAPSHOT_PAGE_SIZE, DataRequest, Filter, MAX_SEARCH_PATTERN_CHARS,
    MAX_SEARCH_PATTERNS, MAX_SNAPSHOT_PAGE_SIZE, Order, Route, RouteError, SegmentRequest, Window,
    parse,
};

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
            type_id: None,
            after: None,
        })
    );
}

#[test]
fn physical_layout_selection_is_available_to_every_generic_row_resource() {
    let Route::History(history) = parse(
        "/api/segments/7/sections/pg_stat_statements/history",
        Some("field=calls&type_id=1002001"),
    )
    .expect("typed history") else {
        panic!("history route");
    };
    assert_eq!(history.type_id, Some(1_002_001));

    let Route::Rows(rows) = parse(
        "/api/segments/7/sections/pg_store_plans/rows",
        Some("field=plan&type_id=1004001"),
    )
    .expect("typed rows") else {
        panic!("rows route");
    };
    assert_eq!(rows.data.type_id, Some(1_004_001));

    let Route::Hour(hour) = parse(
        "/api/hour",
        Some("from=1&to=2&section=pg_stat_statements&field=calls&type_id=1002002"),
    )
    .expect("typed hour") else {
        panic!("hour route");
    };
    assert_eq!(
        hour.series.and_then(|series| series.type_id),
        Some(1_002_002)
    );
}

#[test]
fn snapshot_paging_inputs_enable_one_bounded_page() {
    let Route::Snapshot(ordered) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=total_time&field=total_exec_time&field=calls&by=total_time&by=total_exec_time&by=calls"),
    )
    .expect("candidate order") else {
        panic!("snapshot route");
    };
    assert_eq!(ordered.by, ["total_time", "total_exec_time", "calls"]);
    assert_eq!(ordered.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));
    assert_eq!(ordered.direction, Order::Desc);

    let Route::Snapshot(ascending) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_user_indexes&by=idx_scan&direction=asc"),
    )
    .expect("ascending snapshot") else {
        panic!("snapshot route");
    };
    assert_eq!(ascending.direction, Order::Asc);
    for query in [
        "at=9&section=pg_stat_user_indexes&direction=sideways",
        "at=9&section=pg_stat_user_indexes&direction=asc&direction=desc",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("direction".to_owned())),
        );
    }

    let Route::Snapshot(searched) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=query&search=++slow+query++"),
    )
    .expect("searched page") else {
        panic!("snapshot route");
    };
    assert_eq!(searched.search, ["slow query"]);
    assert_eq!(searched.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));

    let Route::Snapshot(resumed) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&cursor=7%2C0%2C2%2C91%2C101"),
    )
    .expect("resumed page") else {
        panic!("snapshot route");
    };
    assert_eq!(resumed.cursor.as_deref(), Some("7,0,2,91,101"));
    assert_eq!(resumed.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));

    let Route::Snapshot(sized) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&page_size=17"),
    )
    .expect("explicit page size") else {
        panic!("snapshot route");
    };
    assert_eq!(sized.page_size, Some(17));
}

#[test]
fn snapshot_accepts_one_unpaged_exact_locator() {
    let Route::Snapshot(locator) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=query&type_id=1002001&row_ordinal=18446744073709551615"),
    )
    .expect("exact locator") else {
        panic!("snapshot route");
    };
    assert_eq!(locator.type_id, Some(1_002_001));
    assert_eq!(locator.row_ordinal, Some(u64::MAX));
    assert_eq!(locator.page_size, None);

    for query in [
        "at=9&section=pg_stat_statements&row_ordinal=1",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&by=queryid",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&search=slow",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&cursor=7,0,0,1,2",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&page_size=1",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&where.queryid=2",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("row_ordinal".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_page_size_is_positive_and_bounded() {
    let path = "/api/segments/7/snapshot";
    for page_size in [1, MAX_SNAPSHOT_PAGE_SIZE] {
        let query = format!("at=9&section=pg_stat_statements&page_size={page_size}");
        let Route::Snapshot(snapshot) = parse(path, Some(&query)).expect("bounded page size")
        else {
            panic!("snapshot route");
        };
        assert_eq!(snapshot.page_size, Some(page_size));
    }

    for page_size in [0, MAX_SNAPSHOT_PAGE_SIZE + 1] {
        let query = format!("at=9&section=pg_stat_statements&page_size={page_size}");
        assert_eq!(
            parse(path, Some(&query)),
            Err(RouteError::BadParameter("page_size".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_search_is_repeatable_trimmed_and_bounded() {
    let path = "/api/segments/7/snapshot";
    let prefix = "at=9&section=pg_stat_statements&field=query";
    let searches = (0..MAX_SEARCH_PATTERNS)
        .map(|index| format!("search=term{index}"))
        .collect::<Vec<_>>()
        .join("&");
    let query = format!("{prefix}&{searches}");
    let Route::Snapshot(snapshot) = parse(path, Some(&query)).expect("maximum search patterns")
    else {
        panic!("snapshot route");
    };
    assert_eq!(snapshot.search.len(), MAX_SEARCH_PATTERNS);

    let unicode_boundary = "Ж".repeat(MAX_SEARCH_PATTERN_CHARS);
    let query = format!("{prefix}&search=++{unicode_boundary}++");
    let Route::Snapshot(snapshot) = parse(path, Some(&query)).expect("Unicode scalar boundary")
    else {
        panic!("snapshot route");
    };
    assert_eq!(snapshot.search, [unicode_boundary]);

    let too_many = format!("{prefix}&{searches}&search=one-too-many");
    let too_long = "Ж".repeat(MAX_SEARCH_PATTERN_CHARS + 1);
    for query in [
        format!("{prefix}&search="),
        format!("{prefix}&search=+++"),
        format!("{prefix}&search={too_long}"),
        too_many,
    ] {
        assert_eq!(
            parse(path, Some(&query)),
            Err(RouteError::BadParameter("search".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_shares_only_a_projection_between_sections() {
    let Route::Snapshot(projected) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=os_cpu&section=os_meminfo&field=user&field=mem_total"),
    )
    .expect("shared projection") else {
        panic!("snapshot route");
    };
    assert_eq!(projected.sections, ["os_cpu", "os_meminfo"]);
    assert_eq!(projected.fields, ["user", "mem_total"]);
    assert_eq!(projected.page_size, None);
    assert_eq!(projected.cursor, None);
    assert!(projected.search.is_empty());
    assert!(projected.by.is_empty());

    let Route::Snapshot(filtered) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=queryid&where.userid=4"),
    )
    .expect("ordinary filtered snapshot") else {
        panic!("snapshot route");
    };
    assert_eq!(filtered.page_size, None);
    assert_eq!(
        filtered.filters,
        [Filter {
            column: "userid".to_owned(),
            value: "4".to_owned(),
        }]
    );

    for query in [
        "at=9&section=os_cpu&section=os_meminfo&by=user",
        "at=9&section=os_cpu&section=os_meminfo&search=cpu",
        "at=9&section=os_cpu&section=os_meminfo&cursor=7,0,0,1,2",
        "at=9&section=os_cpu&section=os_meminfo&page_size=1",
        "at=9&section=os_cpu&section=os_meminfo&where.cpu_id=-1",
        "at=9&section=os_cpu&section=os_meminfo&type_id=1102001",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("section".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn active_tail_is_one_strict_physical_cursor() {
    let Route::History(request) = parse(
        "/api/segments/7/sections/os_diskstats/history",
        Some("field=reads&after=7%2C18446744073709551615"),
    )
    .expect("active tail") else {
        panic!("history route");
    };
    assert_eq!(
        request.after,
        Some(ActiveCursor {
            segment_id: 7,
            wal_position: u64::MAX,
        })
    );

    for query in [
        "after=8,1",
        "after=7",
        "after=7,1,2",
        "after=7,-1",
        "after=7,1&after=7,2",
    ] {
        assert_eq!(
            parse("/api/segments/7/sections/os_diskstats/history", Some(query),),
            Err(RouteError::BadParameter("after".to_owned())),
            "{query}",
        );
    }
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
fn a_tail_and_page_cursor_remain_separate_physical_inputs() {
    let Route::Rows(request) = parse(
        "/api/segments/7/sections/os_process/rows",
        Some("after=7,100&cursor=7,200,0,1,9"),
    )
    .expect("page inside an active tail") else {
        panic!("rows route");
    };
    assert_eq!(request.data.after.unwrap().wal_position, 100);
    assert_eq!(request.cursor.as_deref(), Some("7,200,0,1,9"));
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
    assert_eq!(
        parse(
            "/api/segments/7/sections/os_cpu/history",
            Some("field=ts&field=ts"),
        ),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(
        parse(
            "/api/segments/7/sections/os_cpu/history",
            Some("where.cpu_id=1&where.cpu_id=2"),
        ),
        Err(RouteError::BadParameter("where".to_owned()))
    );
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
