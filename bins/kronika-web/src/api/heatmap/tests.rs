use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::{Section as _, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Value, json};

use super::execution::{Obs, column_of, interval_start, summed};
use super::{
    HeatmapBatchQuery, HeatmapItemQuery, HeatmapRequest, HeatmapView, NormalizedRanking, prepare,
    prepare_batch,
};
use crate::api::time::TimeRange;

const HOUR: i64 = 1_000_000_000_000;
const SPAN: i64 = 3_600_000_000;
const MINUTE: i64 = 60_000_000;

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
        items: vec![a.clone(), b, a],
    };
    let prepared = prepare_batch(fixture.root.path(), query).expect("prepare");
    let result = prepared.execute(&|| false).expect("execute");
    assert_eq!(prepared.execution_operations(), 1);
    assert_eq!(prepared.row_visits(), 129);
    assert_eq!(result.results.len(), 3);
    assert_eq!(result.results[0], result.results[2]);
    assert_eq!(result.results[0].entity_count, 129);
    assert_eq!(
        result.results[1].coverage.state,
        super::result::CoverageState::NoData
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
        .filter(|column| column.class == kronika_registry::ColumnClass::Label)
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
    assert_eq!(result.results[0].as_of, None);
}

#[test]
fn legacy_http_keeps_ndjson_rows_groups_bands_and_automatic_label_header() {
    let fixture = mount_fixture(129);
    let prepared = prepare(
        fixture.root.path(),
        HeatmapRequest {
            from: fixture.timestamp,
            to: fixture.timestamp,
            section: "os_mountinfo".to_owned(),
            fields: vec!["total_bytes".to_owned()],
            columns: 1,
            top: 1,
            group: vec!["mount_point".to_owned()],
            type_id: None,
        },
    )
    .expect("prepare");
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
}

struct MountFixture {
    root: tempfile::TempDir,
    timestamp: i64,
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
