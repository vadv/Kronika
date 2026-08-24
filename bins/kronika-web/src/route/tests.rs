use super::{
    ActiveCursor, DEFAULT_SNAPSHOT_PAGE_SIZE, DataRequest, Filter, HeatmapRequest,
    MAX_SEARCH_EXPRESSION_CHARS, MAX_SNAPSHOT_PAGE_SIZE, Order, PostgresqlSurface, Route,
    RouteError, SegmentRequest, StatementLens, TableLens, Window, parse,
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
fn relation_hour_series_accepts_only_one_exact_aggregate_scope() {
    let Route::Hour(hour) = parse(
        "/api/hour",
        Some(
            "from=1&to=2&section=pg_stat_user_tables&group=schema&field=seq_scan&field=buffer_hit_pct&where.datid=7&where.schemaname=public",
        ),
    )
    .expect("schema relation series")
    else {
        panic!("hour route");
    };
    let series = hour.series.expect("relation series");
    assert_eq!(series.group, Some(super::RelationGroup::Schema));
    assert_eq!(series.fields, ["seq_scan", "buffer_hit_pct"]);
    assert_eq!(
        series.filters,
        [
            Filter {
                column: "datid".to_owned(),
                value: "7".to_owned(),
            },
            Filter {
                column: "schemaname".to_owned(),
                value: "public".to_owned(),
            },
        ]
    );

    let Route::Hour(hour) = parse(
        "/api/hour",
        Some(
            "from=1&to=2&section=pg_stat_user_indexes&group=tablespace&field=main_fork_bytes&where.tablespace_oid=4294967295",
        ),
    )
    .expect("tablespace relation series")
    else {
        panic!("hour route");
    };
    let series = hour.series.expect("relation series");
    assert_eq!(series.group, Some(super::RelationGroup::Tablespace));
    assert_eq!(series.filters[0].column, "tablespace_oid");
    assert_eq!(series.filters[0].value, "4294967295");

    for query in [
        "from=1&to=2&section=pg_stat_user_tables&group=object&field=seq_scan&where.datid=7",
        "from=1&to=2&section=pg_stat_activity&group=database&field=active&where.datid=7",
        "from=1&to=2&section=pg_stat_user_tables&group=database&field=seq_scan",
        "from=1&to=2&section=pg_stat_user_tables&group=schema&field=seq_scan&where.datid=7",
        "from=1&to=2&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=7&type_id=1013005",
        "from=1&to=2&section=pg_stat_user_tables&group=tablespace&field=seq_scan&where.tablespace_oid=0",
        "from=1&to=2&section=pg_stat_user_tables&group=tablespace&field=seq_scan&where.tablespace_oid=4294967296",
    ] {
        assert!(
            parse("/api/hour", Some(query)).is_err(),
            "invalid aggregate series shape must be rejected: {query}",
        );
    }
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

    let Route::Snapshot(grouped) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_user_tables&group=schema"),
    )
    .expect("relation group") else {
        panic!("snapshot route");
    };
    assert_eq!(grouped.group, Some(super::RelationGroup::Schema));
    assert_eq!(grouped.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));
    for query in [
        "at=9&section=pg_stat_user_indexes&direction=sideways",
        "at=9&section=pg_stat_user_indexes&direction=asc&direction=desc",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("direction".to_owned())),
        );
    }
    assert_eq!(
        parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=pg_stat_activity&group=database"),
        ),
        Err(RouteError::BadParameter("group".to_owned())),
    );
    assert_eq!(
        parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=pg_stat_user_tables&group=object&type_id=1013008"),
        ),
        Err(RouteError::BadParameter("type_id".to_owned())),
    );
    assert_eq!(
        parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=pg_stat_user_tables&section=pg_stat_user_indexes&group=database"),
        ),
        Err(RouteError::BadParameter("section".to_owned())),
    );

    let Route::Snapshot(searched) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=query&search=++slow+query++"),
    )
    .expect("searched page") else {
        panic!("snapshot route");
    };
    assert_eq!(searched.search.as_deref(), Some("slow query"));
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
fn snapshot_postgresql_vocabulary_is_one_typed_surface_request() {
    let Route::Snapshot(statements) = parse(
        "/api/segments/7/snapshot",
        Some(
            "at=9&section=pg_stat_statements&lens=per_call&find=slow+query&order=calls_per_second&direction=asc",
        ),
    )
    .expect("typed Statement surface")
    else {
        panic!("snapshot route");
    };
    let postgresql = statements.postgresql.expect("PostgreSQL surface");
    assert_eq!(
        postgresql.surface,
        PostgresqlSurface::Statements(StatementLens::PerCall)
    );
    assert_eq!(postgresql.order.as_deref(), Some("calls_per_second"));
    assert_eq!(statements.search.as_deref(), Some("slow query"));
    assert!(statements.by.is_empty());
    assert_eq!(statements.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));
    assert_eq!(statements.direction, Order::Asc);

    let Route::Snapshot(tables) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_user_tables&lens=freeze&group=schema"),
    )
    .expect("typed grouped Table surface") else {
        panic!("snapshot route");
    };
    assert_eq!(
        tables.postgresql.expect("PostgreSQL surface").surface,
        PostgresqlSurface::Tables(TableLens::Freeze)
    );

    let Route::Snapshot(objects) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_user_tables&lens=access"),
    )
    .expect("typed object Table surface") else {
        panic!("snapshot route");
    };
    assert_eq!(objects.group, Some(super::RelationGroup::Object));
}

#[test]
fn snapshot_postgresql_vocabulary_does_not_mix_with_legacy_names() {
    let path = "/api/segments/7/snapshot";
    for (query, parameter) in [
        (
            "at=9&section=pg_stat_statements&lens=load&by=calls&order=calls_per_second",
            "order",
        ),
        (
            "at=9&section=pg_stat_statements&lens=load&search=slow&find=slow",
            "find",
        ),
        ("at=9&section=pg_stat_activity&lens=load", "lens"),
        ("at=9&section=pg_stat_database&lens=load", "lens"),
        ("at=9&section=postgres&lens=load", "lens"),
        ("at=9&section=pg_stat_activity&find=pid%3A42", "find"),
        ("at=9&section=pg_stat_database&find=database%3Aapp", "find"),
        (
            "at=9&section=pg_stat_statements&section=pg_store_plans&lens=load",
            "section",
        ),
    ] {
        assert_eq!(
            parse(path, Some(query)),
            Err(RouteError::BadParameter(parameter.to_owned())),
            "{query}",
        );
    }
}

#[test]
fn statement_text_first_match_requires_one_exact_bounded_shape() {
    let path = "/api/segments/7/snapshot";
    let valid = "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A-42&first_match=1";
    let Route::Snapshot(snapshot) = parse(path, Some(valid)).expect("first Statement text") else {
        panic!("snapshot route");
    };
    assert!(snapshot.first_match);
    assert_eq!(snapshot.page_size, Some(1));
    assert_eq!(snapshot.search.as_deref(), Some("query_id:-42"));

    for query in [
        "at=9&section=pg_stat_statements&field=query&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=queryid&field=query&page_size=1&search=query_id%3A42&first_match=1",
        "at=9&section=pg_store_plans&field=query&page_size=1&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=query&by=queryid&page_size=1&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=query&page_size=1&cursor=x&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A42&first_match=0",
        "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A42&first_match=1&first_match=1",
    ] {
        assert_eq!(
            parse(path, Some(query)),
            Err(RouteError::BadParameter("first_match".to_owned())),
            "{query}",
        );
    }
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
fn snapshot_search_is_single_trimmed_and_bounded() {
    let path = "/api/segments/7/snapshot";
    let prefix = "at=9&section=pg_stat_statements&field=query";
    let unicode_boundary = "Ж".repeat(MAX_SEARCH_EXPRESSION_CHARS);
    let query = format!("{prefix}&search=++{unicode_boundary}++");
    let Route::Snapshot(snapshot) = parse(path, Some(&query)).expect("Unicode scalar boundary")
    else {
        panic!("snapshot route");
    };
    assert_eq!(snapshot.search, Some(unicode_boundary));

    let repeated = format!("{prefix}&search=first&search=second");
    let too_long = "Ж".repeat(MAX_SEARCH_EXPRESSION_CHARS + 1);
    for query in [
        format!("{prefix}&search="),
        format!("{prefix}&search=+++"),
        format!("{prefix}&search={too_long}"),
        repeated,
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
    assert!(projected.search.is_none());
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

#[test]
fn a_grouped_heatmap_rejects_labels() {
    match parse(
        "/api/heatmap",
        Some("from=0&to=1&section=os_process&field=utime&group=comm"),
    ) {
        Ok(Route::Heatmap(request)) => assert_eq!(request.group, vec!["comm".to_owned()]),
        other => panic!("expected a heatmap request, got {other:?}"),
    }
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=os_process&field=utime&group=comm&label=cmdline"),
        ),
        Err(RouteError::BadParameter("label".to_owned()))
    );
}

#[test]
fn a_heatmap_cut_may_sum_several_fields() {
    let parsed = parse(
        "/api/heatmap",
        Some(
            "from=0&to=1&section=pg_stat_user_tables&field=n_tup_ins&field=n_tup_upd&field=n_tup_del",
        ),
    );
    match parsed {
        Ok(Route::Heatmap(request)) => assert_eq!(
            request.fields,
            vec![
                "n_tup_ins".to_owned(),
                "n_tup_upd".to_owned(),
                "n_tup_del".to_owned()
            ]
        ),
        other => panic!("expected a heatmap request, got {other:?}"),
    }
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=a&field=a"),
        ),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=a&field=b&field=c&field=d&field=e"),
        ),
        Err(RouteError::BadParameter("field".to_owned()))
    );
}

#[test]
fn a_heatmap_request_needs_a_window_a_section_and_one_field() {
    assert_eq!(
        parse(
            "/api/heatmap",
            Some(
                "from=0&to=3599999999&section=pg_stat_statements&field=wal_bytes&label=datname&label=usename&columns=60&top=25"
            ),
        ),
        Ok(Route::Heatmap(HeatmapRequest {
            from: 0,
            to: 3_599_999_999,
            section: "pg_stat_statements".to_owned(),
            fields: vec!["wal_bytes".to_owned()],
            columns: 60,
            top: 25,
            labels: vec!["datname".to_owned(), "usename".to_owned()],
            group: Vec::new(),
            type_id: None,
        }))
    );
    assert_eq!(
        parse("/api/heatmap", Some("from=0&to=1&section=s")),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(
        parse("/api/heatmap", Some("from=0&to=1&section=s&field=f&top=0")),
        Err(RouteError::BadParameter("top".to_owned()))
    );
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=f&columns=0")
        ),
        Err(RouteError::BadParameter("columns".to_owned()))
    );
    assert_eq!(
        parse("/api/heatmap", Some("from=2&to=1&section=s&field=f")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
}
