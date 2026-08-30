use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::{ColumnClass, Section as _, StrId, Ts, logical_section_name, registry};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Value, json};

use super::execution::{Obs, column_of, interval_start, summed};
use super::{
    HeatmapBatchQuery, HeatmapItemQuery, HeatmapRequest, HeatmapView, NormalizedRanking, prepare,
    prepare_batch,
};
use crate::api::ApiError;
use crate::api::time::TimeRange;

const HOUR: i64 = 1_000_000_000_000;
const SPAN: i64 = 3_600_000_000;
const MINUTE: i64 = 60_000_000;
const REPRODUCED_STATEMENT_IDENTITIES: usize = 5_636;
const STATEMENT_FIELDS: [&str; 5] = [
    "total_exec_time",
    "calls",
    "shared_blks_read",
    "temp_blks_written",
    "wal_bytes",
];

#[test]
fn intervals_cover_the_legacy_window_after_half_open_normalization() {
    let range = TimeRange::new(HOUR, HOUR + SPAN).expect("range");
    assert_eq!(interval_start(range, 12, 0), HOUR);
    assert_eq!(
        interval_start(range, 12, 1).saturating_sub(1),
        HOUR + 300 * 1_000_000 - 1
    );
    assert_eq!(
        interval_start(range, 12, 12).saturating_sub(1),
        HOUR + SPAN - 1
    );
    assert_eq!(column_of(HOUR, range, 60), 0);
    assert_eq!(column_of(HOUR + SPAN - 1, range, 60), 59);
}

#[test]
fn counter_and_gauge_formulas_preserve_null_zero_rate_and_max_rules() {
    let mut counter = Obs::default();
    counter.observe(HOUR, 100.0);
    assert_eq!(counter.cell(true), None);
    counter.observe(HOUR + 30 * MINUTE, 400.0);
    let rate = counter.cell(true).expect("rate");
    assert!((rate - 300.0 / 1_800.0).abs() < 1e-9);
    assert_eq!(counter.total(true), Some(300.0));

    let mut zero = Obs::default();
    zero.observe(HOUR, 5.0);
    zero.observe(HOUR + MINUTE, 5.0);
    assert_eq!(zero.cell(true), Some(0.0));

    let mut reset = Obs::default();
    reset.observe(HOUR, 5.0);
    reset.observe(HOUR + MINUTE, 1.0);
    assert_eq!(reset.cell(true), None);
    assert_eq!(reset.total(true), None);

    let mut gauge = Obs::default();
    gauge.observe(HOUR, 10.0);
    gauge.observe(HOUR + MINUTE, 90.0);
    gauge.observe(HOUR + 2 * MINUTE, 30.0);
    assert_eq!(gauge.cell(false), Some(30.0));
    assert_eq!(gauge.total(false), Some(90.0));
}

#[test]
fn cpu_overview_excludes_the_redundant_aggregate_row() {
    let fixture = cpu_fixture();
    let query = HeatmapBatchQuery {
        range: TimeRange::new(100, 201).expect("CPU range"),
        items: vec![HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: "os_cpu".to_owned(),
                fields: vec!["user".to_owned()],
                top: 1,
            },
            view: HeatmapView::RankingOnly,
        }],
    };
    let prepared = prepare_batch(fixture.root.path(), query).expect("prepare CPU overview");
    let result = prepared.execute(&|| false).expect("execute CPU overview");
    let item = &result.results[0];

    assert_eq!(prepared.row_visits(), 6);
    assert_eq!(item.coverage.window_rows, 4);
    assert_eq!(item.entity_count, 2);
    assert_eq!(item.totals_total, Some(30.0));
    assert_eq!(item.others_total, Some(10.0));
    assert_eq!(item.entities.len(), 1);
    assert_eq!(item.entities[0].identity["cpu_id"], 1);
    assert_eq!(item.entities[0].total, Some(20.0));
}

#[test]
fn late_counter_samples_still_fold_into_their_original_grid_column() {
    let fixture = cumulative_fixture();
    let query = HeatmapBatchQuery {
        range: TimeRange::new(HOUR, HOUR + 60 * MINUTE).expect("range"),
        items: vec![HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: "os_diskstats".to_owned(),
                fields: vec!["reads".to_owned()],
                top: 1,
            },
            view: HeatmapView::Grid {
                columns: 2,
                group: Vec::new(),
                type_id: None,
            },
        }],
    };
    let result = prepare_batch(fixture.root.path(), query)
        .expect("prepare")
        .execute(&|| false)
        .expect("execute");
    let item = &result.results[0];
    let cells = item.entities[0].cells.as_ref().expect("grid cells");
    assert_eq!(item.out_of_order, 2);
    assert_eq!(cells[0], Some(50.0 / 300.0));
    assert_eq!(cells[1], None);
}

#[test]
fn a_summed_recipe_uses_present_fields_and_stays_null_without_any() {
    let contract = kronika_registry::contract(1_013_008).expect("tables contract");
    let cells = contract
        .columns
        .iter()
        .map(|column| match column.name {
            "n_tup_ins" => kronika_reader::Cell::I64(5),
            "n_tup_upd" => kronika_reader::Cell::I64(7),
            _ => kronika_reader::Cell::Null,
        })
        .collect();
    let row = kronika_reader::Row::new(contract, cells);
    assert_eq!(
        summed(&row, &["n_tup_ins", "n_tup_upd", "n_tup_del"]),
        Some(12.0)
    );
    assert_eq!(summed(&row, &["n_tup_del"]), None);
}

#[test]
fn shared_section_batch_and_duplicates_decode_each_row_once() {
    let fixture = mount_fixture(129);
    let a = ranking("total_bytes", 3);
    let b = ranking("free_bytes", 2);
    let query = HeatmapBatchQuery {
        range: TimeRange::new(fixture.timestamp, fixture.timestamp + 1).expect("range"),
        items: vec![a.clone(), b.clone(), a.clone()],
    };
    let prepared = prepare_batch(fixture.root.path(), query).expect("prepare");
    let result = prepared.execute(&|| false).expect("execute");
    assert_eq!(prepared.execution_operations(), 1);
    assert_eq!(prepared.row_visits(), 129);
    assert_eq!(result.results.len(), 3);
    assert_eq!(
        result
            .results
            .iter()
            .map(|item| &item.ranking)
            .collect::<Vec<_>>(),
        [&a.ranking, &b.ranking, &a.ranking]
    );
    assert_eq!(result.results[0], result.results[2]);
    assert_eq!(result.results[0].entity_count, 129);
    assert_eq!(
        result.results[1].coverage.state,
        super::result::CoverageState::NoData
    );
    assert_eq!(
        serde_json::to_value(&result.results[1]).expect("serialize no-data result"),
        json!({
            "ranking": {
                "section": "os_mountinfo",
                "fields": ["free_bytes"],
                "top": 2,
            },
            "coverage": {
                "state": "no_data",
                "window_rows": "129",
            },
            "class": "gauge",
            "unit": "bytes",
            "entities": [],
            "totals_total": null,
            "others_total": null,
            "entity_count": "0",
            "out_of_order": "0",
        })
    );
}

#[test]
fn high_cardinality_statement_rankings_share_identity_labels_and_one_scan() {
    let fixture = statement_fixture(REPRODUCED_STATEMENT_IDENTITIES);
    let mut independent = Vec::new();
    for field in STATEMENT_FIELDS {
        let prepared = prepare_batch(
            fixture.root.path(),
            statement_batch(std::slice::from_ref(&statement_ranking(field, 10))),
        )
        .expect("prepare independent ranking");
        let result = prepared.execute(&|| false).expect("independent ranking");
        assert_eq!(prepared.row_visits(), 11_272);
        assert_eq!(prepared.retained_identities(), 5_636);
        assert_eq!(prepared.metric_fold_slots(), 5_636);
        assert_statement_ranking(&result.results[0], field, 10);
        independent.push(result.results[0].clone());
    }

    let rankings = STATEMENT_FIELDS
        .iter()
        .map(|field| statement_ranking(field, 10))
        .collect::<Vec<_>>();
    let prepared = prepare_batch(fixture.root.path(), statement_batch(&rankings))
        .expect("prepare combined rankings");
    let result = prepared.execute(&|| false).expect("combined rankings");
    assert_eq!(result.results, independent);
    assert_eq!(prepared.execution_operations(), 1);
    assert_eq!(prepared.row_visits(), 11_272);
    assert_eq!(prepared.retained_identities(), 5_636);
    assert_eq!(prepared.metric_fold_slots(), 28_180);
    assert_eq!(
        prepared.retained_label_slots(),
        u64::try_from(REPRODUCED_STATEMENT_IDENTITIES * statement_label_count())
            .expect("label-slot count")
    );
    let top_one = STATEMENT_FIELDS
        .iter()
        .map(|field| statement_ranking(field, 1))
        .collect::<Vec<_>>();
    let top_one = prepare_batch(fixture.root.path(), statement_batch(&top_one))
        .expect("prepare top-one rankings");
    top_one.execute(&|| false).expect("top-one rankings");
    assert_eq!(top_one.row_visits(), prepared.row_visits());
    assert_eq!(
        top_one.retained_identities(),
        prepared.retained_identities()
    );
    assert_eq!(
        top_one.retained_label_slots(),
        prepared.retained_label_slots()
    );
    assert_eq!(top_one.metric_fold_slots(), prepared.metric_fold_slots());
}

#[test]
fn statement_overview_omits_query_and_keeps_the_latest_locator_in_one_scan() {
    let fixture = statement_fixture(2);
    let prepared = prepare_batch(
        fixture.root.path(),
        statement_batch(&[statement_ranking("total_exec_time", 1)]),
    )
    .expect("prepare statement ranking");
    let result = prepared
        .execute(&|| false)
        .expect("execute statement ranking");
    let encoded = serde_json::to_string(&result).expect("serialize statement ranking");
    let entity = &result.results[0].entities[0];

    assert_eq!(prepared.row_visits(), 4);
    assert_eq!(prepared.retained_identities(), 2);
    assert_eq!(prepared.retained_label_slots(), 12);
    assert!(!encoded.contains("statement-"));
    assert_eq!(entity.identity["query_id"], "1");
    assert!(!entity.labels.contains_key("query"));
    assert_eq!(
        serde_json::to_value(&entity.detail_locator).expect("serialize detail locator"),
        json!({
            "section": "pg_stat_statements",
            "segment_id": HOUR.to_string(),
            "at": "200",
            "type_id": PgStatStatementsV2::CONTRACT.type_id.get().to_string(),
            "row_ordinal": "3",
            "identity": {
                "queryid": "1",
                "userid": "72",
                "dbid": "73",
            },
        })
    );
}

#[test]
fn prepared_statement_ranking_keeps_its_captured_active_prefix() {
    let mut fixture = ActiveStatementFixture::new();
    fixture.append(1, 10.0);
    let query = statement_batch(&[statement_ranking("total_exec_time", 1)]);
    let captured_prepared =
        prepare_batch(fixture.root.path(), query.clone()).expect("captured prepare");

    fixture.append(2, 100.0);
    let captured = captured_prepared
        .execute(&|| false)
        .expect("captured execution");
    assert_eq!(captured_prepared.row_visits(), 2);
    assert_eq!(captured.results[0].entities[0].identity["query_id"], "1");
    assert!(!captured.results[0].entities[0].labels.contains_key("query"));
    assert_eq!(
        captured.results[0].entities[0].detail_locator.identity["queryid"],
        json!("1")
    );

    let current_prepared = prepare_batch(fixture.root.path(), query).expect("current prepare");
    let current = current_prepared
        .execute(&|| false)
        .expect("current execution");
    assert_eq!(current_prepared.row_visits(), 4);
    assert_eq!(current.results[0].entities[0].identity["query_id"], "2");
    assert!(!current.results[0].entities[0].labels.contains_key("query"));
    assert_eq!(
        current.results[0].entities[0].detail_locator.identity["queryid"],
        json!("2")
    );
}

#[test]
fn automatic_labels_are_complete_and_latest_non_null_wins() {
    let fixture = labelled_mount_fixture();
    let query = HeatmapBatchQuery {
        range: TimeRange::new(fixture.timestamp, fixture.timestamp + 3).expect("range"),
        items: vec![ranking("total_bytes", 1)],
    };
    let result = prepare_batch(fixture.root.path(), query)
        .expect("prepare")
        .execute(&|| false)
        .expect("execute");
    let entity = &result.results[0].entities[0];
    let expected: Vec<String> = kronika_registry::contract(OsMountinfo::CONTRACT.type_id.get())
        .expect("contract")
        .columns
        .iter()
        .filter(|column| column.class == ColumnClass::Label)
        .map(|column| column.name.to_owned())
        .collect();
    assert_eq!(entity.labels.keys().count(), expected.len());
    for name in expected {
        assert!(entity.labels.contains_key(&name), "missing label {name}");
    }
    assert_eq!(entity.labels["fstype"], "ignored");
    assert_eq!(entity.labels["source"], "/same");
}

#[test]
fn equal_timestamp_labels_use_the_winning_source_segment_dictionary() {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    for offset in 0..=1 {
        let address = SegmentAddress::new(SegmentId::new(HOUR + offset).expect("segment id"))
            .expect("segment address");
        let mut interner = Interner::new(DictLimits::default());
        let mount_point = StrId(
            if offset == 0 {
                interner.intern(b"/same")
            } else {
                interner.intern_blob(b"/same")
            }
            .expect("mount")
            .get(),
        );
        let fstype = StrId(
            if offset == 0 {
                interner.intern_blob(b"same")
            } else {
                interner.intern(b"same")
            }
            .expect("filesystem")
            .get(),
        );
        let mut buffers = SectionBuffers::new();
        buffers
            .push(OsMountinfo {
                ts: Ts(HOUR),
                major: 8,
                minor: 0,
                mount_point,
                root: mount_point,
                fstype,
                source: mount_point,
                is_k8s_infra: false,
                total_bytes: (offset == 1).then_some(100),
                free_bytes: None,
                total_inodes: None,
                available_inodes: None,
                scope: 0,
            })
            .expect("mount row");
        let dictionary = dict::encode(interner.window()).expect("dictionary");
        let part = buffers.flush(&dictionary).expect("encode").expect("part");
        journal.append(address.id, &part).expect("append");
        write_segment(&journal, &owner, address).expect("finish segment");
        if offset == 0 {
            journal.reset().expect("reset journal");
        }
    }
    drop(journal);
    drop(owner);

    let result = prepare_batch(
        root.path(),
        HeatmapBatchQuery {
            range: TimeRange::new(HOUR, HOUR + 1).expect("range"),
            items: vec![ranking("total_bytes", 1)],
        },
    )
    .expect("prepare")
    .execute(&|| false)
    .expect("execute");
    let entity = &result.results[0].entities[0];
    assert_eq!(entity.identity["mount_point"], "/same");
    assert_eq!(entity.labels["fstype"], "same");
}

#[test]
fn invalid_item_reports_its_zero_based_index_and_returns_no_prefix() {
    let fixture = mount_fixture(1);
    let query = HeatmapBatchQuery {
        range: TimeRange::new(fixture.timestamp, fixture.timestamp + 1).expect("range"),
        items: vec![ranking("total_bytes", 1), ranking("missing", 1)],
    };
    let error = prepare_batch(fixture.root.path(), query)
        .err()
        .expect("invalid item");
    assert_eq!(error.ranking_index(), 1);
    assert!(error.to_string().contains("rankings[1]"));
}

#[test]
fn legacy_prepare_preserves_specific_api_errors() {
    let request = |section: &str, fields: &[&str], to| HeatmapRequest {
        from: HOUR,
        to,
        section: section.to_owned(),
        fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        columns: 1,
        top: 1,
        group: Vec::new(),
        type_id: None,
    };
    let root = std::env::temp_dir();

    let section = prepare(&root, request("missing", &["total_bytes"], HOUR))
        .err()
        .expect("unknown section");
    assert!(matches!(section, ApiError::NoSuchSection));

    let column = prepare(&root, request("os_mountinfo", &["missing"], HOUR))
        .err()
        .expect("unknown column");
    assert!(matches!(column, ApiError::NoSuchColumn(field) if field == "missing"));

    let units = prepare(
        &root,
        request("os_mountinfo", &["total_bytes", "total_inodes"], HOUR),
    )
    .err()
    .expect("mixed units");
    assert!(matches!(units, ApiError::MixedUnits(fields) if fields == "total_bytes+total_inodes"));

    let overflow = prepare(&root, request("os_mountinfo", &["total_bytes"], i64::MAX))
        .err()
        .expect("inclusive end overflow");
    assert!(matches!(overflow, ApiError::BadFilter(parameter) if parameter == "to"));
}

#[test]
fn more_than_eight_rankings_have_no_count_cap() {
    let fixture = mount_fixture(1);
    let query = HeatmapBatchQuery {
        range: TimeRange::new(fixture.timestamp, fixture.timestamp + 1).expect("range"),
        items: (0..9).map(|_| ranking("total_bytes", 1)).collect(),
    };
    let result = prepare_batch(fixture.root.path(), query)
        .expect("prepare")
        .execute(&|| false)
        .expect("execute");
    assert_eq!(result.results.len(), 9);
}

#[test]
fn exact_to_is_excluded_from_the_product_range() {
    let fixture = mount_fixture(1);
    let query = HeatmapBatchQuery {
        range: TimeRange::new(fixture.timestamp - 1, fixture.timestamp).expect("range"),
        items: vec![ranking("total_bytes", 1)],
    };
    let result = prepare_batch(fixture.root.path(), query)
        .expect("prepare")
        .execute(&|| false)
        .expect("execute");
    assert_eq!(result.results[0].entity_count, 0);
    assert_eq!(
        result.results[0].coverage.state,
        super::result::CoverageState::NoData
    );
}

#[test]
fn legacy_http_keeps_ndjson_rows_groups_bands_and_automatic_label_header() {
    let fixture = mount_fixture(129);
    let request = HeatmapRequest {
        from: fixture.timestamp,
        to: fixture.timestamp,
        section: "os_mountinfo".to_owned(),
        fields: vec!["total_bytes".to_owned()],
        columns: 1,
        top: 1,
        group: vec!["mount_point".to_owned()],
        type_id: None,
    };
    let shared = prepare_batch(
        fixture.root.path(),
        request.clone().normalize().expect("normalize HTTP request"),
    )
    .expect("prepare shared item")
    .execute(&|| false)
    .expect("execute shared item");
    let item = &shared.results[0];
    let grid = item.grid.as_ref().expect("grid result");
    let prepared = prepare(fixture.root.path(), request).expect("prepare HTTP item");
    let mut bytes = Vec::new();
    prepared
        .stream(
            &mut |record| {
                bytes.extend(record);
                true
            },
            &|| false,
        )
        .expect("stream");
    let records: Vec<Value> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("record"))
        .collect();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0]["record"], "heatmap");
    assert!(
        records[0]["labels"]
            .as_array()
            .is_some_and(|labels| !labels.is_empty())
    );
    assert_eq!(records[1]["record"], "heatmap_row");
    assert_eq!(records[1]["identity"], json!(["/mount-128"]));
    assert_eq!(records[1]["members"], 1);
    assert_eq!(records[2]["band"], "totals");
    assert_eq!(records[3]["band"], "others");
    assert_eq!(records[0]["section"], item.ranking.section);
    assert_eq!(records[0]["fields"], json!(item.ranking.fields));
    assert_eq!(records[0]["entity_count"], item.entity_count);
    assert_eq!(records[0]["top"], grid.groups.len());
    assert_eq!(records[1]["identity"], json!(grid.groups[0].values));
    assert_eq!(records[1]["members"], grid.groups[0].members);
    assert_eq!(records[1]["total"], json!(grid.groups[0].total));
    assert_eq!(records[1]["cells"], json!(grid.groups[0].cells));
    assert_eq!(records[2]["total"], json!(grid.totals.total));
    assert_eq!(records[2]["cells"], json!(grid.totals.cells));
    assert_eq!(records[3]["total"], json!(grid.others.total));
    assert_eq!(records[3]["cells"], json!(grid.others.cells));
}

struct MountFixture {
    root: tempfile::TempDir,
    timestamp: i64,
}

struct ActiveStatementFixture {
    root: tempfile::TempDir,
    _owner: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl ActiveStatementFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("fixture directory");
        let data_root = DataRoot::open(root.path()).expect("data root");
        let owner = data_root
            .acquire_writer(LayoutLimits::default())
            .expect("writer");
        let journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
        let address = SegmentAddress::new(SegmentId::new(HOUR).expect("segment id"))
            .expect("segment address");
        Self {
            root,
            _owner: owner,
            journal,
            address,
        }
    }

    fn append(&mut self, query_id: usize, total_exec_time: f64) {
        let mut interner = Interner::new(DictLimits::default());
        let datname = StrId(interner.intern(b"active_db").expect("database label").get());
        let usename = StrId(interner.intern(b"active_role").expect("role label").get());
        let query = StrId(
            interner
                .intern(format!("active-{query_id}").as_bytes())
                .expect("query label")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        buffers
            .push(statement_row(100, query_id, datname, usename, query))
            .expect("baseline statement");
        let mut current = statement_row(200, query_id, datname, usename, query);
        current.total_exec_time = total_exec_time;
        buffers.push(current).expect("current statement");
        let dictionary = dict::encode(interner.window()).expect("active dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode active statements")
            .expect("nonempty active statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append active statements");
    }
}

fn ranking(field: &str, top: usize) -> HeatmapItemQuery {
    HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "os_mountinfo".to_owned(),
            fields: vec![field.to_owned()],
            top,
        },
        view: HeatmapView::RankingOnly,
    }
}

fn statement_ranking(field: &str, top: usize) -> HeatmapItemQuery {
    HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "pg_stat_statements".to_owned(),
            fields: vec![field.to_owned()],
            top,
        },
        view: HeatmapView::RankingOnly,
    }
}

fn statement_batch(items: &[HeatmapItemQuery]) -> HeatmapBatchQuery {
    HeatmapBatchQuery {
        range: TimeRange::new(100, 201).expect("statement range"),
        items: items.to_vec(),
    }
}

fn statement_label_count() -> usize {
    let mut labels = Vec::new();
    for contract in registry().iter().filter(|contract| {
        logical_section_name(contract.type_id.get()) == Some("pg_stat_statements")
    }) {
        for column in contract.columns {
            if column.class == ColumnClass::Label
                && !crate::api::row_key::is_detail_text("pg_stat_statements", column.name)
                && !labels.contains(&column.name)
            {
                labels.push(column.name);
            }
        }
    }
    labels.len()
}

fn statement_integer_value(field: &str, query_id: usize) -> usize {
    let rotation = match field {
        "total_exec_time" => 0,
        "calls" => 997,
        "shared_blks_read" => 1_994,
        "temp_blks_written" => 2_991,
        "wal_bytes" => 3_988,
        _ => panic!("unexpected statement field {field}"),
    };
    (query_id + rotation) % REPRODUCED_STATEMENT_IDENTITIES + 1
}

fn statement_value(field: &str, query_id: usize) -> f64 {
    f64::from(
        u32::try_from(statement_integer_value(field, query_id)).expect("fixture value fits u32"),
    )
}

fn assert_statement_ranking(item: &super::result::HeatmapItemResult, field: &str, top: usize) {
    let mut expected = (0..REPRODUCED_STATEMENT_IDENTITIES)
        .map(|query_id| (query_id, statement_value(field, query_id)))
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| right.1.partial_cmp(&left.1).expect("finite fixture values"));
    assert_eq!(item.entity_count, 5_636);
    assert_eq!(item.entities.len(), top);
    let count =
        f64::from(u32::try_from(REPRODUCED_STATEMENT_IDENTITIES).expect("fixture count fits u32"));
    assert_eq!(item.totals_total, Some(count * (count + 1.0) / 2.0));
    assert_eq!(
        item.others_total,
        Some(
            expected
                .iter()
                .skip(top)
                .map(|(_query_id, value)| value)
                .sum()
        )
    );
    for (entity, (query_id, value)) in item.entities.iter().zip(expected) {
        assert_eq!(entity.identity["query_id"], query_id.to_string());
        assert_eq!(entity.total, Some(value));
        assert_eq!(entity.labels.len(), statement_label_count());
        assert_eq!(entity.labels["queryid"], query_id.to_string());
        assert_eq!(entity.labels["userid"], 72);
        assert_eq!(entity.labels["dbid"], 73);
        assert!(entity.labels["toplevel"].is_null());
        assert_eq!(entity.labels["datname"], "fixture_db");
        assert_eq!(entity.labels["usename"], "fixture_role");
        assert!(!entity.labels.contains_key("query"));
        assert_eq!(entity.detail_locator.section, "pg_stat_statements");
        assert_eq!(entity.detail_locator.segment_id, json!(HOUR.to_string()));
        assert_eq!(entity.detail_locator.at, json!("200"));
        assert_eq!(
            entity.detail_locator.type_id,
            json!(PgStatStatementsV2::CONTRACT.type_id.get().to_string())
        );
        assert_eq!(
            entity.detail_locator.row_ordinal,
            json!((query_id * 2 + 1).to_string())
        );
        assert_eq!(
            entity.detail_locator.identity["queryid"],
            json!(query_id.to_string())
        );
        assert_eq!(entity.detail_locator.identity["userid"], "72");
        assert_eq!(entity.detail_locator.identity["dbid"], "73");
    }
}

fn statement_fixture(identities: usize) -> MountFixture {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let datname = StrId(
        interner
            .intern(b"fixture_db")
            .expect("database label")
            .get(),
    );
    let usename = StrId(interner.intern(b"fixture_role").expect("role label").get());
    let mut buffers = SectionBuffers::new();
    for query_id in 0..identities {
        let query = StrId(
            interner
                .intern(format!("statement-{query_id}").as_bytes())
                .expect("query label")
                .get(),
        );
        for timestamp in [100, 200] {
            let current = timestamp == 200;
            let value = |field| {
                if current {
                    statement_value(field, query_id)
                } else {
                    0.0
                }
            };
            let integer = |field| {
                if current {
                    i64::try_from(statement_integer_value(field, query_id))
                        .expect("fixture value fits i64")
                } else {
                    0
                }
            };
            let mut row = statement_row(timestamp, query_id, datname, usename, query);
            row.calls = integer("calls");
            row.total_exec_time = value("total_exec_time");
            row.shared_blks_read = integer("shared_blks_read");
            row.temp_blks_written = integer("temp_blks_written");
            row.wal_bytes = integer("wal_bytes");
            buffers.push(row).expect("statement row fits");
        }
    }
    let dictionary = dict::encode(interner.window()).expect("statement dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode statements")
        .expect("nonempty statements");
    journal
        .append(SegmentId::new(HOUR).expect("segment id"), &part)
        .expect("append statements");
    drop(journal);
    drop(owner);
    MountFixture {
        root,
        timestamp: 100,
    }
}

fn statement_row(
    timestamp: i64,
    query_id: usize,
    datname: StrId,
    usename: StrId,
    query: StrId,
) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(timestamp),
        queryid: Some(i64::try_from(query_id).expect("query id")),
        userid: 72,
        dbid: 73,
        datname: Some(datname),
        usename: Some(usename),
        query: Some(query),
        calls: 0,
        rows: 0,
        plans: 0,
        total_exec_time: 0.0,
        total_plan_time: 0.0,
        min_exec_time: 0.0,
        max_exec_time: 0.0,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
        stddev_plan_time: 0.0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        wal_records: 0,
        wal_fpi: 0,
        wal_bytes: 0,
    }
}

fn mount_fixture(rows: i32) -> MountFixture {
    build_mount_fixture((0..rows).map(|index| {
        (
            HOUR,
            index,
            format!("/mount-{index:03}"),
            format!("fs-{index:03}"),
            Some(i64::from(index)),
        )
    }))
}

fn labelled_mount_fixture() -> MountFixture {
    build_mount_fixture([
        (HOUR, 1, "/same".to_owned(), "oldfs".to_owned(), Some(10)),
        (HOUR + 1, 1, "/same".to_owned(), "newfs".to_owned(), None),
        (
            HOUR + 2,
            1,
            "/same".to_owned(),
            "ignored".to_owned(),
            Some(20),
        ),
    ])
}

fn cumulative_fixture() -> MountFixture {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let device = StrId(interner.intern(b"sda").expect("device").get());
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    for (index, (timestamp, reads)) in [
        (HOUR + 40 * MINUTE, 400),
        (HOUR + 5 * MINUTE, 50),
        (HOUR + 10 * MINUTE, 100),
    ]
    .into_iter()
    .enumerate()
    {
        let mut buffers = SectionBuffers::new();
        buffers
            .push(OsDiskstats {
                ts: Ts(timestamp),
                major: 8,
                minor: 0,
                device,
                reads,
                r_merged: 0,
                read_sectors: 0,
                read_time_ms: 0,
                writes: 0,
                w_merged: 0,
                write_sectors: 0,
                write_time_ms: 0,
                io_in_progress: 0,
                io_time_ms: 0,
                io_weighted_time_ms: 0,
                discards: None,
                d_merged: None,
                discard_sectors: None,
                discard_time_ms: None,
                flushes: None,
                flush_time_ms: None,
                scope: 0,
            })
            .expect("diskstats row");
        let encoded_dictionary = if index == 0 {
            dictionary.as_slice()
        } else {
            &[]
        };
        let part = buffers
            .flush(encoded_dictionary)
            .expect("encode")
            .expect("part");
        journal
            .append(SegmentId::new(HOUR).expect("segment id"), &part)
            .expect("append");
    }
    drop(journal);
    drop(owner);
    MountFixture {
        root,
        timestamp: HOUR,
    }
}

fn cpu_fixture() -> MountFixture {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut buffers = SectionBuffers::new();
    for (timestamp, aggregate, first, second) in [(100, 0, 0, 0), (200, 30, 10, 20)] {
        for (cpu_id, user) in [(-1, aggregate), (0, first), (1, second)] {
            buffers
                .push(OsCpu {
                    ts: Ts(timestamp),
                    cpu_id,
                    user,
                    nice: 0,
                    system: 0,
                    idle: 0,
                    iowait: 0,
                    irq: 0,
                    softirq: 0,
                    steal: 0,
                    guest: 0,
                    guest_nice: 0,
                    scope: 0,
                })
                .expect("CPU row");
        }
    }
    let part = buffers
        .flush(&[])
        .expect("encode CPU rows")
        .expect("nonempty CPU rows");
    journal
        .append(SegmentId::new(HOUR).expect("segment id"), &part)
        .expect("append CPU rows");
    drop(journal);
    drop(owner);
    MountFixture {
        root,
        timestamp: 100,
    }
}

fn build_mount_fixture(
    rows: impl IntoIterator<Item = (i64, i32, String, String, Option<i64>)>,
) -> MountFixture {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let mut buffers = SectionBuffers::new();
    for (timestamp, major, mount, filesystem, total_bytes) in rows {
        let mount_point = StrId(interner.intern(mount.as_bytes()).expect("mount").get());
        let fstype = StrId(
            interner
                .intern(filesystem.as_bytes())
                .expect("filesystem")
                .get(),
        );
        buffers
            .push(OsMountinfo {
                ts: Ts(timestamp),
                major,
                minor: 0,
                mount_point,
                root: mount_point,
                fstype,
                source: mount_point,
                is_k8s_infra: false,
                total_bytes,
                free_bytes: None,
                total_inodes: None,
                available_inodes: None,
                scope: 0,
            })
            .expect("mount row");
    }
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let part = buffers.flush(&dictionary).expect("encode").expect("part");
    journal
        .append(SegmentId::new(HOUR).expect("segment id"), &part)
        .expect("append");
    drop(journal);
    drop(owner);
    MountFixture {
        root,
        timestamp: HOUR,
    }
}
