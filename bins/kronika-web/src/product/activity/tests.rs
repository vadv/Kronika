use std::time::{Duration, Instant};

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::pg_stat_activity::{PgStatActivityV1, PgStatActivityV2, PgStatActivityV3};
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};

use super::*;

const HOUR_START: i64 = 1_709_164_800_000_000;
const OBSERVED: i64 = HOUR_START + 2_000_000;

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary data root");
        let root = DataRoot::open(directory.path()).expect("open data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");
        let address = SegmentAddress::new(SegmentId::new(HOUR_START).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            _writer: writer,
            journal,
            address,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn append_union(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let active = intern(&mut interner, "active");
        let idle = intern(&mut interner, "idle");
        let transactional_idle = intern(&mut interner, "idle in transaction");
        let database = intern(&mut interner, "appdb");
        let role = intern(&mut interner, "operator");
        let application = intern(&mut interner, "psql");
        let client = intern(&mut interner, "127.0.0.1");
        let backend = intern(&mut interner, "client backend");
        let system_backend = intern(&mut interner, "autovacuum worker");
        let parallel_apply_backend = intern(&mut interner, "parallel apply worker");
        let wait_type = intern(&mut interner, "IO");
        let wait_event = intern(&mut interner, "DataFileRead");
        let query = intern(&mut interner, &"q".repeat(161));
        let mut buffers = SectionBuffers::new();
        buffers
            .push(v1(
                OBSERVED,
                10,
                active,
                database,
                role,
                application,
                client,
                system_backend,
                wait_type,
                wait_event,
                query,
            ))
            .expect("V1 row fits");
        buffers
            .push(v2(
                OBSERVED,
                20,
                idle,
                database,
                role,
                application,
                client,
                backend,
                wait_type,
                wait_event,
                query,
            ))
            .expect("V2 row fits");
        buffers
            .push(v3(
                OBSERVED,
                30,
                transactional_idle,
                database,
                role,
                application,
                client,
                parallel_apply_backend,
                wait_type,
                wait_event,
                query,
            ))
            .expect("V3 row fits");
        buffers
            .push(v3(
                OBSERVED - 1,
                99,
                active,
                database,
                role,
                application,
                client,
                backend,
                wait_type,
                wait_event,
                query,
            ))
            .expect("older row fits");
        buffers
            .push(v3(
                OBSERVED + 100,
                100,
                active,
                database,
                role,
                application,
                client,
                backend,
                wait_type,
                wait_event,
                query,
            ))
            .expect("future row fits");
        self.append(buffers, &interner);
    }

    fn append_v3_pids(&mut self, pids: &[i32]) {
        self.append_v3_at_pids(OBSERVED, pids);
    }

    fn append_v3_at_pids(&mut self, observed_at: i64, pids: &[i32]) {
        let mut interner = Interner::new(DictLimits::default());
        let active = intern(&mut interner, "active");
        let database = intern(&mut interner, "appdb");
        let role = intern(&mut interner, "operator");
        let application = intern(&mut interner, "psql");
        let client = intern(&mut interner, "local");
        let backend = intern(&mut interner, "client backend");
        let wait_type = intern(&mut interner, "CPU");
        let wait_event = intern(&mut interner, "Running");
        let query = intern(&mut interner, "select 1");
        let mut buffers = SectionBuffers::new();
        for pid in pids {
            buffers
                .push(v3(
                    observed_at,
                    *pid,
                    active,
                    database,
                    role,
                    application,
                    client,
                    backend,
                    wait_type,
                    wait_event,
                    query,
                ))
                .expect("V3 row fits");
        }
        self.append(buffers, &interner);
    }

    fn append(&mut self, mut buffers: SectionBuffers, interner: &Interner) {
        let dictionary = dict::encode(interner.window()).expect("encode dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode Activity part")
            .expect("nonempty Activity part");
        self.journal
            .append(self.address.id, &part)
            .expect("append Activity part");
    }
}

fn intern(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern fixture text")
            .get(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the fixture spells every Activity text column explicitly"
)]
fn v1(
    ts: i64,
    pid: i32,
    state: StrId,
    datname: StrId,
    usename: StrId,
    application_name: StrId,
    client_addr: StrId,
    backend_type: StrId,
    wait_event_type: StrId,
    wait_event: StrId,
    query: StrId,
) -> PgStatActivityV1 {
    PgStatActivityV1 {
        ts: Ts(ts),
        pid,
        datname: Some(datname),
        usename: Some(usename),
        application_name,
        client_addr,
        backend_type,
        state: Some(state),
        wait_event_type: Some(wait_event_type),
        wait_event: Some(wait_event),
        query: Some(query),
        backend_xid_age: Some(11),
        backend_xmin_age: Some(12),
        backend_start: Ts(ts - 8_000),
        xact_start: Some(Ts(ts - 4_000)),
        query_start: Some(Ts(ts - 2_000)),
        state_change: Some(Ts(ts - 1_000)),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the fixture spells every Activity text column explicitly"
)]
fn v2(
    ts: i64,
    pid: i32,
    state: StrId,
    datname: StrId,
    usename: StrId,
    application_name: StrId,
    client_addr: StrId,
    backend_type: StrId,
    wait_event_type: StrId,
    wait_event: StrId,
    query: StrId,
) -> PgStatActivityV2 {
    PgStatActivityV2 {
        ts: Ts(ts),
        pid,
        leader_pid: (pid > 1).then_some(pid - 1),
        datname: Some(datname),
        usename: Some(usename),
        application_name,
        client_addr,
        backend_type,
        state: Some(state),
        wait_event_type: Some(wait_event_type),
        wait_event: Some(wait_event),
        query: Some(query),
        backend_xid_age: Some(21),
        backend_xmin_age: Some(22),
        backend_start: Ts(ts - 8_000),
        xact_start: Some(Ts(ts - 4_000)),
        query_start: Some(Ts(ts - 2_000)),
        state_change: Some(Ts(ts - 1_000)),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the fixture spells every Activity text column explicitly"
)]
fn v3(
    ts: i64,
    pid: i32,
    state: StrId,
    datname: StrId,
    usename: StrId,
    application_name: StrId,
    client_addr: StrId,
    backend_type: StrId,
    wait_event_type: StrId,
    wait_event: StrId,
    query: StrId,
) -> PgStatActivityV3 {
    PgStatActivityV3 {
        ts: Ts(ts),
        pid,
        leader_pid: (pid > 1).then_some(pid - 1),
        datid: Some(42),
        datname: Some(datname),
        usename: Some(usename),
        application_name,
        client_addr,
        backend_type,
        state: Some(state),
        wait_event_type: Some(wait_event_type),
        wait_event: Some(wait_event),
        query: Some(query),
        query_id: Some(-7),
        backend_xid_age: Some(31),
        backend_xmin_age: Some(32),
        backend_start: Ts(ts - 8_000),
        xact_start: Some(Ts(ts - 4_000)),
        query_start: Some(Ts(ts - 2_000)),
        state_change: Some(Ts(ts - 1_000)),
    }
}

fn execution() -> Execution {
    Execution::new(|| false, Instant::now() + Duration::from_secs(30))
}

fn args() -> ActivityArgs {
    ActivityArgs {
        at: (OBSERVED + 50).to_string(),
        filter: None,
        sort: Some(ActivitySort::Pid),
        direction: Some(Direction::Asc),
        page_size: Some(200),
        cursor: None,
    }
}

#[test]
fn runtime_shape_rejects_explicit_null_for_every_nonnullable_optional_field() {
    for field in ["filter", "sort", "direction", "page_size", "cursor"] {
        let mut value = serde_json::json!({"at": OBSERVED.to_string()});
        value
            .as_object_mut()
            .expect("object")
            .insert(field.to_owned(), Value::Null);
        assert!(
            ActivityArgs::from_value(value).is_err(),
            "top-level {field} accepted null"
        );
    }

    for field in [
        "text",
        "pid",
        "query_id",
        "database",
        "role",
        "application",
        "client",
        "backend_type",
        "state",
        "wait_type",
        "wait_event",
    ] {
        let mut clause = serde_json::Map::new();
        clause.insert(field.to_owned(), Value::Null);
        let value = serde_json::json!({
            "at": OBSERVED.to_string(),
            "filter": [Value::Object(clause)]
        });
        assert!(
            ActivityArgs::from_value(value).is_err(),
            "clause field {field} accepted null"
        );
    }

    for field in ["any_of", "all_of"] {
        let mut predicate = serde_json::Map::new();
        predicate.insert(field.to_owned(), Value::Null);
        let value = serde_json::json!({
            "at": OBSERVED.to_string(),
            "filter": [{"text": Value::Object(predicate)}]
        });
        assert!(
            ActivityArgs::from_value(value).is_err(),
            "text matcher field {field} accepted null"
        );
    }
}

#[test]
fn normalization_accepts_both_signed_extremes_without_hour_overflow() {
    let minimum = normalize_activity(ActivityArgs {
        at: i64::MIN.to_string(),
        ..args()
    })
    .expect("minimum at normalizes");
    assert!(minimum.hour_start_wide <= i128::from(i64::MIN));
    assert!(minimum.hour_end_exclusive_wide > i128::from(i64::MIN));

    let maximum = normalize_activity(ActivityArgs {
        at: i64::MAX.to_string(),
        ..args()
    })
    .expect("maximum at normalizes");
    assert!(maximum.hour_start_wide <= i128::from(i64::MAX));
    assert!(maximum.hour_end_exclusive_wide > i128::from(i64::MAX));

    for invalid in ["", "00", "-0", "+1", "01", "9223372036854775808"] {
        assert!(
            normalize_activity(ActivityArgs {
                at: invalid.to_owned(),
                ..args()
            })
            .is_err(),
            "{invalid:?} must be rejected"
        );
    }
}

#[test]
fn filter_distinguishes_omission_empty_and_query_id_only() {
    let omitted = normalize_activity(args()).expect("omitted filter");
    assert!(omitted.filter.matches(&sample_row(1)));

    let empty = normalize_activity(ActivityArgs {
        filter: Some(Vec::new()),
        ..args()
    })
    .expect("empty filter is valid");
    assert!(!empty.filter.matches(&sample_row(1)));
    assert_ne!(omitted.query_binding, empty.query_binding);

    let query_id = normalize_activity(ActivityArgs {
        filter: Some(vec![ActivityClauseArgs {
            query_id: Some(QueryIdMatchArgs {
                any_of: vec!["-7".to_owned()],
            }),
            ..ActivityClauseArgs::default()
        }]),
        ..args()
    })
    .expect("query-ID-only clause");
    assert!(query_id.filter.matches(&sample_row(1)));
}

#[test]
fn clause_accepts_exactly_eight_named_properties_with_query_id_counted_once() {
    let text = || TextMatchArgs {
        any_of: Some(vec!["x".to_owned()]),
        all_of: None,
    };
    let clause = ActivityClauseArgs {
        text: Some(text()),
        pid: Some(PidMatchArgs { any_of: vec![1] }),
        query_id: Some(QueryIdMatchArgs {
            any_of: vec!["-7".to_owned()],
        }),
        database: Some(text()),
        role: Some(text()),
        application: Some(text()),
        client: Some(text()),
        backend_type: Some(text()),
        ..ActivityClauseArgs::default()
    };
    assert!(normalize_clause(clause).is_ok());
}

#[test]
fn text_matcher_handles_cross_field_all_unicode_and_line_terminators() {
    let predicate = normalize_text_match(TextMatchArgs {
        any_of: Some(vec!["😀?".to_owned()]),
        all_of: Some(vec!["APPDB".to_owned(), "line*tail".to_owned()]),
    })
    .expect("valid matcher");
    let fields = [Some("😀x"), Some("appdb"), Some("line\nmid\ntail")];
    assert!(predicate.matches_fields(&fields));
    assert!(!predicate.matches_fields(&[Some("😀xy"), Some("appdb")]));

    let combining = GlobPattern::new("e?".to_owned()).expect("combining matcher");
    assert!(combining.matches("e\u{301}"));
    assert!(!combining.matches("e"));
    let punctuation = GlobPattern::new("a.b[1]".to_owned()).expect("literal punctuation");
    assert!(punctuation.matches("prefix A.B[1] suffix"));
}

#[test]
fn query_preview_and_duration_null_gates_are_exact() {
    let exact = "x".repeat(160);
    assert_eq!(shorten_query(exact.clone()), exact);
    let shortened = shorten_query(format!("{}y", "x".repeat(160)));
    assert_eq!(shortened.chars().count(), 161);
    assert!(shortened.ends_with('…'));

    assert_eq!(duration_ms(10_500, Some(10_000)), Some(0.5));
    assert_eq!(duration_ms(10_000, Some(10_001)), None);
    assert_eq!(duration_ms(10_000, Some(0)), None);
    assert_eq!(
        duration_ms(i64::MAX, Some(1)),
        Some(9_223_372_036_854_775.0)
    );

    let mut row = sample_row(1);
    row.state = Some("idle".to_owned());
    row.query_duration_ms = None;
    row.state_duration_ms = None;
    assert_eq!(row.query_duration_ms, None);
    assert_eq!(row.state_duration_ms, None);
}

#[test]
fn all_sort_pairs_keep_nullable_values_last_and_the_default_secondary_is_exact() {
    let sorts = [
        ActivitySort::Pid,
        ActivitySort::Database,
        ActivitySort::Role,
        ActivitySort::QueryPreview,
        ActivitySort::QueryDurationMs,
        ActivitySort::TransactionDurationMs,
        ActivitySort::Application,
        ActivitySort::Client,
        ActivitySort::State,
        ActivitySort::WaitType,
        ActivitySort::WaitEvent,
        ActivitySort::BackendType,
    ];
    for sort in sorts {
        for direction in [Direction::Asc, Direction::Desc] {
            let mut rows = ranked_rows();
            sort_rows(&mut rows, sort, direction, &execution()).expect("sort completes");
            if matches!(
                sort,
                ActivitySort::Database
                    | ActivitySort::Role
                    | ActivitySort::QueryPreview
                    | ActivitySort::QueryDurationMs
                    | ActivitySort::TransactionDurationMs
                    | ActivitySort::State
                    | ActivitySort::WaitType
                    | ActivitySort::WaitEvent
            ) {
                assert_eq!(rows.last().map(|row| row.row.pid), Some(3));
            }
        }
    }

    let mut rows = ranked_rows();
    rows[0].row.query_duration_ms = Some(10.0);
    rows[1].row.query_duration_ms = Some(10.0);
    rows[0].row.transaction_duration_ms = Some(1.0);
    rows[1].row.transaction_duration_ms = Some(2.0);
    sort_rows(
        &mut rows,
        ActivitySort::QueryDurationMs,
        Direction::Desc,
        &execution(),
    )
    .expect("default sort");
    assert_eq!(rows[0].row.pid, 2, "transaction duration breaks desc tie");

    let mut rows = ranked_rows();
    rows[0].row.query_duration_ms = Some(10.0);
    rows[1].row.query_duration_ms = Some(10.0);
    rows[0].row.transaction_duration_ms = Some(100.0);
    rows[1].row.transaction_duration_ms = Some(1.0);
    sort_rows(
        &mut rows,
        ActivitySort::QueryDurationMs,
        Direction::Asc,
        &execution(),
    )
    .expect("ascending query duration sort");
    assert_eq!(
        rows[0].row.pid, 1,
        "ascending uses PID directly after primary"
    );
}

#[test]
fn executor_selects_one_latest_observation_and_unions_pg10_through_pg18_layouts() {
    let mut fixture = Fixture::new();
    fixture.append_union();
    let query = normalize_activity(args()).expect("query normalizes");
    let result = execute_activity(
        fixture.root(),
        &query,
        &PageKey::derive(b"account"),
        &execution(),
    )
    .expect("Activity executes");
    assert_eq!(
        result.observed_at.as_deref(),
        Some(OBSERVED.to_string().as_str())
    );
    assert_eq!(
        result.rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
        [10, 20, 30]
    );
    assert_eq!(result.rows[0].leader_pid, None);
    assert_eq!(result.rows[0].datid, None);
    assert_eq!(result.rows[0].query_id, None);
    assert_eq!(result.rows[1].leader_pid, Some(19));
    assert_eq!(result.rows[1].datid, None);
    assert_eq!(result.rows[1].query_id, None);
    assert_eq!(result.rows[2].datid, Some(42));
    assert_eq!(result.rows[2].query_id.as_deref(), Some("-7"));
    assert_eq!(result.rows[2].backend_type, "parallel apply worker");
    assert_eq!(
        result.rows[0]
            .query_preview
            .as_ref()
            .map(|text| text.chars().count()),
        Some(161)
    );
    assert_eq!(result.rows[0].query_duration_ms, Some(2.0));
    assert_eq!(result.rows[0].backend_type, "autovacuum worker");
    assert_eq!(result.rows[1].state.as_deref(), Some("idle"));
    assert_eq!(result.rows[1].query_duration_ms, None);
    assert_eq!(result.rows[1].state_duration_ms, None);
    assert_eq!(result.rows[2].state_duration_ms, Some(1.0));
}

#[test]
fn no_observation_and_zero_match_are_successes_with_distinct_observation_metadata() {
    let mut fixture = Fixture::new();
    fixture.append_union();
    let no_observation = execute_activity(
        fixture.root(),
        &normalize_activity(ActivityArgs {
            at: (HOUR_START - 1).to_string(),
            ..args()
        })
        .expect("prior hour query"),
        &PageKey::derive(b"account"),
        &execution(),
    )
    .expect("empty hour succeeds");
    assert_eq!(no_observation.observed_at, None);
    assert!(no_observation.rows.is_empty());

    let zero_match = execute_activity(
        fixture.root(),
        &normalize_activity(ActivityArgs {
            filter: Some(Vec::new()),
            ..args()
        })
        .expect("match-none query"),
        &PageKey::derive(b"account"),
        &execution(),
    )
    .expect("zero match succeeds");
    assert_eq!(zero_match.observed_at, Some(OBSERVED.to_string()));
    assert!(zero_match.rows.is_empty());
}

#[test]
fn more_than_one_thousand_rows_traverse_without_duplicates_or_omissions() {
    let mut fixture = Fixture::new();
    let expected: Vec<i32> = (1..=1_205).collect();
    fixture.append_v3_pids(&expected);
    let key = PageKey::derive(b"large-activity-traversal");
    let mut cursor = None;
    let mut actual = Vec::new();
    loop {
        let query = normalize_activity(ActivityArgs {
            page_size: Some(137),
            cursor: cursor.clone(),
            ..args()
        })
        .expect("page query");
        let result =
            execute_activity(fixture.root(), &query, &key, &execution()).expect("Activity page");
        assert_eq!(result.observed_at, Some(OBSERVED.to_string()));
        actual.extend(result.rows.into_iter().map(|row| row.pid));
        cursor = result.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(actual, expected);
}

#[test]
fn executor_filters_then_sorts_then_pages_the_retained_population() {
    let mut fixture = Fixture::new();
    fixture.append_v3_pids(&(1..=10).collect::<Vec<_>>());
    let key = PageKey::derive(b"activity-filter-sort-page");
    let filter = Some(vec![ActivityClauseArgs {
        pid: Some(PidMatchArgs {
            any_of: vec![2, 4, 6, 8, 10],
        }),
        ..ActivityClauseArgs::default()
    }]);
    let first_query = normalize_activity(ActivityArgs {
        filter: filter.clone(),
        sort: Some(ActivitySort::Pid),
        direction: Some(Direction::Desc),
        page_size: Some(2),
        ..args()
    })
    .expect("first page query");
    let first = execute_activity(fixture.root(), &first_query, &key, &execution())
        .expect("first filtered page");
    assert_eq!(
        first.rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
        [10, 8]
    );

    let second_query = normalize_activity(ActivityArgs {
        filter,
        sort: Some(ActivitySort::Pid),
        direction: Some(Direction::Desc),
        page_size: Some(2),
        cursor: first.next_cursor,
        ..args()
    })
    .expect("second page query");
    let second = execute_activity(fixture.root(), &second_query, &key, &execution())
        .expect("second filtered page");
    assert_eq!(
        second.rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
        [6, 4]
    );
    assert!(second.next_cursor.is_some());
}

#[test]
fn cursor_pins_active_prefix_and_rejects_changed_query_or_authentication() {
    let mut fixture = Fixture::new();
    fixture.append_v3_pids(&[2, 3, 4]);
    let key = PageKey::derive(b"account");
    let first_query = normalize_activity(ActivityArgs {
        page_size: Some(1),
        ..args()
    })
    .expect("first query");
    let first =
        execute_activity(fixture.root(), &first_query, &key, &execution()).expect("first page");
    assert_eq!(
        first.rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
        [2]
    );
    let cursor = first.next_cursor.expect("continuation");
    assert!(cursor.len() <= 4_096);

    fixture.append_v3_at_pids(OBSERVED + 25, &[1]);
    let second_query = normalize_activity(ActivityArgs {
        page_size: Some(1),
        cursor: Some(cursor.clone()),
        ..args()
    })
    .expect("continuation query");
    let second = execute_activity(fixture.root(), &second_query, &key, &execution())
        .expect("pinned second page");
    assert_eq!(
        second.rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
        [3]
    );

    let changed_queries = [
        ActivityArgs {
            at: (OBSERVED + 51).to_string(),
            page_size: Some(1),
            cursor: Some(cursor.clone()),
            ..args()
        },
        ActivityArgs {
            filter: Some(Vec::new()),
            page_size: Some(1),
            cursor: Some(cursor.clone()),
            ..args()
        },
        ActivityArgs {
            sort: Some(ActivitySort::Database),
            page_size: Some(1),
            cursor: Some(cursor.clone()),
            ..args()
        },
        ActivityArgs {
            direction: Some(Direction::Desc),
            page_size: Some(1),
            cursor: Some(cursor.clone()),
            ..args()
        },
        ActivityArgs {
            page_size: Some(2),
            cursor: Some(cursor.clone()),
            ..args()
        },
    ];
    for changed_args in changed_queries {
        let changed = normalize_activity(changed_args).expect("changed query normalizes");
        assert_eq!(
            execute_activity(fixture.root(), &changed, &key, &execution())
                .expect_err("changed binding is rejected")
                .code(),
            "invalid_arguments"
        );
    }
    assert_eq!(
        execute_activity(
            fixture.root(),
            &second_query,
            &PageKey::derive(b"other account"),
            &execution()
        )
        .expect_err("different authentication is rejected")
        .code(),
        "invalid_arguments"
    );

    let fresh = execute_activity(fixture.root(), &first_query, &key, &execution())
        .expect("fresh listing sees growth");
    assert_eq!(fresh.observed_at, Some((OBSERVED + 25).to_string()));
    assert_eq!(fresh.rows[0].pid, 1);
}

#[test]
fn executor_reports_cancellation_and_deadline() {
    let fixture = Fixture::new();
    let query = normalize_activity(args()).expect("query");
    let cancelled = Execution::new(|| true, Instant::now() + Duration::from_secs(1));
    assert_eq!(
        execute_activity(
            fixture.root(),
            &query,
            &PageKey::derive(b"account"),
            &cancelled
        )
        .expect_err("cancelled")
        .code(),
        "cancelled"
    );
    let expired = Execution::new(|| false, Instant::now() - Duration::from_secs(1));
    assert_eq!(
        execute_activity(
            fixture.root(),
            &query,
            &PageKey::derive(b"account"),
            &expired
        )
        .expect_err("deadline")
        .code(),
        "deadline_exceeded"
    );
}

fn sample_row(pid: i32) -> ActivityRow {
    ActivityRow {
        observed_at: OBSERVED.to_string(),
        pid,
        leader_pid: None,
        datid: Some(42),
        datname: Some("appdb".to_owned()),
        usename: Some("operator".to_owned()),
        application_name: "psql".to_owned(),
        client_addr: "local".to_owned(),
        backend_type: "client backend".to_owned(),
        state: Some("active".to_owned()),
        wait_event_type: Some("CPU".to_owned()),
        wait_event: Some("Running".to_owned()),
        query_preview: Some("select 1".to_owned()),
        query_id: Some("-7".to_owned()),
        backend_xid_age: Some("10".to_owned()),
        backend_xmin_age: Some("20".to_owned()),
        backend_start: (OBSERVED - 10_000).to_string(),
        xact_start: Some((OBSERVED - 5_000).to_string()),
        query_start: Some((OBSERVED - 2_000).to_string()),
        state_change: Some((OBSERVED - 1_000).to_string()),
        backend_age_ms: Some(10.0),
        query_duration_ms: Some(f64::from(pid)),
        transaction_duration_ms: Some(5.0 - f64::from(pid)),
        state_duration_ms: Some(1.0),
    }
}

fn ranked_rows() -> Vec<RankedActivityRow> {
    let mut first = sample_row(1);
    first.datname = Some("alpha".to_owned());
    first.usename = Some("alpha".to_owned());
    first.query_preview = Some("alpha".to_owned());
    first.application_name = "alpha".to_owned();
    first.client_addr = "alpha".to_owned();
    first.state = Some("alpha".to_owned());
    first.wait_event_type = Some("alpha".to_owned());
    first.wait_event = Some("alpha".to_owned());
    first.backend_type = "alpha".to_owned();
    let mut second = sample_row(2);
    second.datname = Some("beta".to_owned());
    second.usename = Some("beta".to_owned());
    second.query_preview = Some("beta".to_owned());
    second.application_name = "beta".to_owned();
    second.client_addr = "beta".to_owned();
    second.state = Some("beta".to_owned());
    second.wait_event_type = Some("beta".to_owned());
    second.wait_event = Some("beta".to_owned());
    second.backend_type = "beta".to_owned();
    let mut third = sample_row(3);
    third.datname = None;
    third.usename = None;
    third.query_preview = None;
    third.query_duration_ms = None;
    third.transaction_duration_ms = None;
    third.state = None;
    third.wait_event_type = None;
    third.wait_event = None;
    [first, second, third]
        .into_iter()
        .enumerate()
        .map(|(source, row)| RankedActivityRow {
            row,
            coordinate: RowCoordinate {
                source,
                layout: 1_001_004,
                ordinal: 0,
            },
        })
        .collect()
}
