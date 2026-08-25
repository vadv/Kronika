use super::{
    ActiveCursor, DEFAULT_SNAPSHOT_PAGE_SIZE, DataRequest, Filter, MAX_SEARCH_EXPRESSION_CHARS,
    MAX_SNAPSHOT_PAGE_SIZE, Order, Route, RouteError, SegmentRequest, Window, parse,
    parse_activity, parse_top_activity,
};
use crate::product::activity::{ActivityArgs, ActivitySort, normalize_activity};
use crate::product::page::Direction;
use crate::product::top_activity::{
    Metric, RelationLevel, Surface, metric_definitions, surface_definitions,
};
use serde_json::json;

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
fn top_activity_http_uses_semantic_defaults_and_rejects_physical_grammar() {
    let query = parse_top_activity(Some("hour=0&surface=postgresql_tables"))
        .expect("semantic top-activity query");
    assert_eq!(query.hour().start(), 0);
    assert_eq!(query.selection().surface(), Surface::PostgreSqlTables);
    assert_eq!(query.selection().metric(), Metric::Writes);
    assert_eq!(query.selection().level(), Some(RelationLevel::Object));
    assert_eq!(query.top().get(), 25);

    assert_eq!(
        parse_top_activity(Some(
            "hour=0&surface=processes&section=os_process&field=utime&group=comm"
        )),
        Err(RouteError::BadParameter("section".to_owned()))
    );
    assert_eq!(
        parse_top_activity(Some("hour=1&surface=processes")),
        Err(RouteError::BadParameter("hour".to_owned()))
    );
    assert_eq!(
        parse_top_activity(Some("hour=0&surface=processes&level=object")),
        Err(RouteError::BadParameter("level".to_owned()))
    );
}

#[test]
fn top_activity_http_accepts_all_61_shipped_selections_and_rejects_every_cross_pair() {
    let mut accepted = 0;
    for definition in metric_definitions() {
        let levels: &[&str] = if matches!(
            definition.surface,
            Surface::PostgreSqlTables | Surface::PostgreSqlIndexes
        ) {
            &["object", "schema", "database", "tablespace"]
        } else {
            &[""]
        };
        for level in levels {
            let level = if level.is_empty() {
                String::new()
            } else {
                format!("&level={level}")
            };
            let raw = format!(
                "hour=0&surface={}&metric={}{}&top=100",
                definition.surface.as_str(),
                definition.metric.as_str(),
                level
            );
            let query = parse_top_activity(Some(&raw)).expect("valid shipped selection");
            assert_eq!(query.selection().surface(), definition.surface);
            assert_eq!(query.selection().metric(), definition.metric);
            accepted += 1;
        }
    }
    assert_eq!(accepted, 61);

    let mut rejected = 0;
    for surface in surface_definitions() {
        for metric in Metric::ALL {
            if metric_definitions().iter().any(|definition| {
                definition.surface == surface.surface && definition.metric == metric
            }) {
                continue;
            }
            let raw = format!(
                "hour=0&surface={}&metric={}",
                surface.surface.as_str(),
                metric.as_str()
            );
            assert_eq!(
                parse_top_activity(Some(&raw)),
                Err(RouteError::BadParameter("metric".to_owned()))
            );
            rejected += 1;
        }
    }
    assert_eq!(rejected, 219);
}

#[test]
fn activity_http_and_typed_arguments_share_one_normalized_query() {
    let parsed = parse_activity(Some(concat!(
        "at=3600000001&",
        "filter=%5B%7B%22database%22%3A%7B%22any_of%22%3A%5B%22prod%2A%22%5D%7D%7D%5D&",
        "sort=database&direction=asc&page_size=50&cursor=pc1_test"
    )))
    .expect("semantic Activity query");
    let arguments = ActivityArgs::from_value(json!({
        "at": "3600000001",
        "filter": [{"database": {"any_of": ["prod*"]}}],
        "sort": "database",
        "direction": "asc",
        "page_size": 50,
        "cursor": "pc1_test"
    }))
    .expect("typed Activity arguments");
    let normalized = normalize_activity(arguments).expect("normalized Activity query");
    assert_eq!(parsed, normalized);
}

#[test]
fn activity_http_has_v6_defaults_and_all_semantic_sort_directions() {
    let defaults = parse_activity(Some("at=0")).expect("default Activity query");
    assert_eq!(defaults.at, 0);
    assert_eq!(defaults.sort, ActivitySort::QueryDurationMs);
    assert_eq!(defaults.direction, Direction::Desc);
    assert_eq!(defaults.page.page_size, 200);
    assert!(defaults.page.cursor.is_none());

    let sorts = [
        "pid",
        "database",
        "role",
        "query_preview",
        "query_duration_ms",
        "transaction_duration_ms",
        "application",
        "client",
        "state",
        "wait_type",
        "wait_event",
        "backend_type",
    ];
    for sort in sorts {
        for direction in ["asc", "desc"] {
            parse_activity(Some(&format!("at=0&sort={sort}&direction={direction}")))
                .unwrap_or_else(|error| panic!("{sort}/{direction} rejected: {error}"));
        }
    }
}

#[test]
fn activity_http_rejects_missing_duplicate_and_invalid_product_arguments() {
    assert_eq!(
        parse_activity(None),
        Err(RouteError::BadParameter("at".to_owned()))
    );
    assert_eq!(
        parse_activity(Some("at=0&at=0")),
        Err(RouteError::BadParameter("at".to_owned()))
    );
    assert_eq!(
        parse_activity(Some("at=0&page_size=0")),
        Err(RouteError::BadParameter("page_size".to_owned()))
    );
    assert_eq!(
        parse_activity(Some("at=0&filter=null")),
        Err(RouteError::BadParameter("filter".to_owned()))
    );
    assert_eq!(
        parse_activity(Some("at=0&section=pg_stat_activity")),
        Err(RouteError::BadParameter("section".to_owned()))
    );
}
