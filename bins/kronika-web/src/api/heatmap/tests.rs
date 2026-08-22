use serde_json::{Value, json};

use super::{
    Fold, GroupedFill, Obs, column_of, entity_key_into, interval_end, interval_start,
    ungrouped_batch_rows,
};

fn entity_key(type_id: u32, identity: &[Value]) -> String {
    let mut key = String::new();
    entity_key_into(&mut key, type_id, identity);
    key
}

const HOUR: i64 = 1_000_000_000_000;
const SPAN: i64 = 3_600_000_000;
const MINUTE: i64 = 60_000_000;

fn end() -> i64 {
    HOUR + SPAN - 1
}

fn identity(name: &str) -> Vec<Value> {
    vec![json!(name)]
}

#[test]
fn intervals_carry_exact_boundaries_and_cover_the_window() {
    assert_eq!(interval_start(HOUR, end(), 12, 0), HOUR);
    assert_eq!(interval_end(HOUR, end(), 12, 0), HOUR + 300 * 1_000_000 - 1);
    assert_eq!(interval_end(HOUR, end(), 12, 11), end());
    assert_eq!(column_of(HOUR, HOUR, end(), 60), 0);
    assert_eq!(column_of(end(), HOUR, end(), 60), 59);
}

#[test]
fn ungrouped_batch_size_is_bounded_at_the_public_column_limit() {
    assert!(ungrouped_batch_rows(60, 2) >= 100);
    let public_max_batch = ungrouped_batch_rows(1_440, 2);
    assert!(public_max_batch > 0);
    assert!(public_max_batch < 500);
}

#[test]
fn a_counter_cell_is_the_delta_over_the_observed_elapsed_time() {
    let mut observed = Obs::default();
    observed.observe(HOUR, 100.0);
    observed.observe(HOUR + 30 * MINUTE, 400.0);
    let rate = observed.cell(true).unwrap_or_default();
    assert!((rate - 300.0 / 1_800.0).abs() < 1e-9);
    assert_eq!(observed.total(true), Some(300.0));
}

#[test]
fn counter_null_rules_one_sample_zero_duration_negative_delta() {
    let mut one = Obs::default();
    one.observe(HOUR, 5.0);
    assert_eq!(one.cell(true), None);

    let mut torn = Obs::default();
    torn.observe(HOUR, 1.0);
    torn.observe(HOUR, 2.0);
    assert_eq!(torn.cell(true), None);

    let mut reset = Obs::default();
    reset.observe(HOUR, 500.0);
    reset.observe(HOUR + MINUTE, 100.0);
    assert_eq!(reset.cell(true), None);
    assert_eq!(reset.total(true), None);

    let mut idle = Obs::default();
    idle.observe(HOUR, 500.0);
    idle.observe(HOUR + MINUTE, 500.0);
    assert_eq!(idle.cell(true), Some(0.0));
}

#[test]
fn a_gauge_cell_is_the_last_sample_and_ranks_by_the_maximum() {
    let mut observed = Obs::default();
    observed.observe(HOUR, 10.0);
    observed.observe(HOUR + MINUTE, 90.0);
    observed.observe(HOUR + 2 * MINUTE, 30.0);
    assert_eq!(observed.cell(false), Some(30.0));
    assert_eq!(observed.total(false), Some(90.0));
}

#[test]
fn the_fold_ranks_by_the_whole_window_and_aggregates_the_rest() {
    let mut fold = Fold::new(HOUR, end(), 1, true);
    for (name, per_minute) in [("a", 3_600.0), ("b", 1_800.0), ("c", 900.0)] {
        fold.observe(1, &identity(name), None, HOUR, Some(0.0));
        fold.observe(
            1,
            &identity(name),
            None,
            HOUR + 30 * MINUTE,
            Some(per_minute),
        );
    }
    let ranked = fold.finish(2);
    assert_eq!(ranked.entity_count, 3);
    assert_eq!(ranked.rows.len(), 2);
    assert_eq!(ranked.rows[0].identity, identity("a"));
    assert_eq!(ranked.rows[0].total, Some(3_600.0));
    assert_eq!(ranked.rows[1].identity, identity("b"));
    assert_eq!(ranked.totals_total, Some(6_300.0));
    assert_eq!(ranked.others_total, Some(900.0));
    let totals = ranked.totals[0].value().unwrap_or_default();
    assert!((totals - 3.5).abs() < 1e-9);
}

#[test]
fn the_totals_band_folds_each_finished_column_in_recording_order() {
    let mut fold = Fold::new(HOUR, end(), 2, true);
    fold.observe(1, &identity("a"), None, HOUR, Some(0.0));
    fold.observe(1, &identity("a"), None, HOUR + 15 * MINUTE, Some(900.0));
    fold.observe(1, &identity("a"), None, HOUR + 40 * MINUTE, Some(900.0));
    fold.observe(1, &identity("a"), None, HOUR + 55 * MINUTE, Some(1_800.0));
    let ranked = fold.finish(1);
    // Minute 40 measures the span back to minute 15, whose middle is minute
    // 27, so its flat 900 lands in the first column and spreads over the 40
    // observed minutes. Only minute 55 reaches the second column.
    let first = ranked.totals[0].value().unwrap_or_default();
    let second = ranked.totals[1].value().unwrap_or_default();
    assert!((first - 0.375).abs() < 1e-9);
    assert!((second - 1.0).abs() < 1e-9);
    assert_eq!(ranked.out_of_order, 0);
}

#[test]
fn a_sample_for_a_finished_column_is_counted_not_folded() {
    let mut fold = Fold::new(HOUR, end(), 2, true);
    fold.observe(1, &identity("a"), None, HOUR + 40 * MINUTE, Some(100.0));
    fold.observe(1, &identity("a"), None, HOUR + 5 * MINUTE, Some(1.0));
    let ranked = fold.finish(1);
    assert_eq!(ranked.out_of_order, 1);
}

#[test]
fn samples_outside_the_window_and_null_values_are_ignored() {
    let mut fold = Fold::new(HOUR, end(), 1, true);
    fold.observe(1, &identity("a"), None, HOUR - 1, Some(0.0));
    fold.observe(1, &identity("a"), None, HOUR, Some(100.0));
    fold.observe(1, &identity("a"), None, HOUR + MINUTE, None);
    fold.observe(1, &identity("a"), None, HOUR + 2 * MINUTE, Some(200.0));
    fold.observe(1, &identity("a"), None, end() + 1, Some(900.0));
    let ranked = fold.finish(1);
    assert_eq!(ranked.rows[0].total, Some(100.0));
}

#[test]
fn entities_with_a_null_ranking_value_sort_after_real_totals() {
    let mut fold = Fold::new(HOUR, end(), 1, true);
    fold.observe(1, &identity("reset"), None, HOUR, Some(900.0));
    fold.observe(1, &identity("reset"), None, HOUR + MINUTE, Some(100.0));
    fold.observe(1, &identity("steady"), None, HOUR, Some(0.0));
    fold.observe(1, &identity("steady"), None, HOUR + MINUTE, Some(60.0));
    let ranked = fold.finish(5);
    assert_eq!(ranked.rows[0].identity, identity("steady"));
    assert_eq!(ranked.rows[1].total, None);
}

#[test]
fn a_sparse_cadence_draws_each_span_in_the_column_holding_its_middle() {
    // One sample per column, as tables and indexes record. Twelve samples
    // measure eleven spans, and each is drawn where its middle falls: the
    // first column reads, the last has nothing recorded across it yet.
    let mut fold = Fold::new(HOUR, end(), 12, true);
    for column in 0..12_i64 {
        #[expect(clippy::cast_precision_loss, reason = "twelve small columns")]
        fold.observe(
            1,
            &identity("a"),
            None,
            HOUR + column * 5 * MINUTE,
            Some(600.0 * column as f64),
        );
    }
    let ranked = fold.finish(1);
    for column in 0..11 {
        let cell = ranked.totals[column].value().unwrap_or_default();
        assert!((cell - 2.0).abs() < 1e-9, "column {column}: {cell}");
    }
    assert_eq!(ranked.totals[11].value(), None);
}

#[test]
fn a_cadence_as_coarse_as_a_column_survives_collection_drift() {
    // Tables are recorded every five minutes and the ledger draws twelve
    // five-minute columns. Collection drifts either side of every boundary,
    // which pairs two samples in one column and leaves the next one empty as
    // long as a sample is drawn where it was taken. Drawn where its span's
    // middle falls, each reading lands in its own column.
    let seconds = [
        0, 301, 553, 959, 1_142, 1_558, 1_743, 2_144, 2_351, 2_751, 2_952, 3_330, 3_540,
    ];
    let mut fold = Fold::new(HOUR, end(), 12, true);
    for at in seconds {
        #[expect(clippy::cast_precision_loss, reason = "an hour of whole seconds")]
        fold.observe(
            1,
            &identity("a"),
            None,
            HOUR + at * 1_000_000,
            Some(at as f64),
        );
    }
    let ranked = fold.finish(1);
    for column in 0..12 {
        let cell = ranked.totals[column].value().unwrap_or_default();
        assert!((cell - 1.0).abs() < 1e-9, "column {column}: {cell}");
    }
}

#[test]
fn identity_streams_stay_separate_per_layout() {
    assert_ne!(entity_key(1, &identity("a")), entity_key(2, &identity("a")));
    assert_ne!(
        entity_key(1, &[json!("a"), json!(null)]),
        entity_key(1, &[json!("a"), json!("null")])
    );
}

#[test]
fn a_summed_cut_adds_the_present_fields_and_stays_null_without_any() {
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
        super::summed(&row, &["n_tup_ins", "n_tup_upd", "n_tup_del"]),
        Some(12.0)
    );
    assert_eq!(super::summed(&row, &["n_tup_del"]), None);
}

#[test]
fn a_grouped_ranking_sums_identities_under_one_value_and_counts_members() {
    let mut fold = Fold::new(HOUR, end(), 2, true);
    // Two workers of one command, one of them dying mid-hour, plus a loner.
    let samples = [
        ("101", "postgres", HOUR, 0.0),
        ("101", "postgres", HOUR + 15 * MINUTE, 900.0),
        ("102", "postgres", HOUR, 0.0),
        ("102", "postgres", HOUR + 40 * MINUTE, 1_200.0),
        ("7", "cron", HOUR, 0.0),
        ("7", "cron", HOUR + 40 * MINUTE, 240.0),
    ];
    for &(entity, group, ts, value) in &samples {
        fold.observe(
            1,
            &identity(entity),
            Some(vec![json!(group)]),
            ts,
            Some(value),
        );
    }
    let grouped = fold.finish_grouped(1);
    assert_eq!(grouped.group_count, 2);
    assert_eq!(grouped.rows.len(), 1);
    let top = &grouped.rows[0];
    assert_eq!(top.values, vec![json!("postgres")]);
    assert_eq!(top.members, 2);
    assert_eq!(top.total, Some(2_100.0));
    let mut fill = GroupedFill::new(HOUR, end(), 2, true, &grouped.rows);
    for &(entity, group, ts, value) in &samples {
        fill.observe(1, &identity(entity), &[json!(group)], ts, Some(value));
    }
    let filled = fill.finish();
    let first = filled.rows[0][0].value().unwrap_or_default();
    assert!(first > 0.0);
    assert_eq!(grouped.others_total, Some(240.0));
    // cron was read twice, at the start of the hour and at minute 40; that one
    // span has its middle at minute 20, so it is drawn in the first column and
    // nothing reaches the second.
    let others_first = filled.others[0].value().unwrap_or_default();
    assert!(others_first > 0.0);
    assert_eq!(filled.others[1].value(), None);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "shared fixture setup covers grouped and batched ungrouped output"
)]
fn a_high_cardinality_dictionary_is_resolved_for_the_whole_plan() {
    use kronika_format::DictLimits;
    use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
    use kronika_registry::os_mountinfo::OsMountinfo;
    use kronika_registry::{StrId, Ts};
    use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};

    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let mut buffers = SectionBuffers::new();
    let ts = 1_709_164_800_000_000;
    for index in 0..129 {
        let name = format!("/mount-{index:03}");
        let mount_point = StrId(interner.intern(name.as_bytes()).expect("intern").get());
        buffers
            .push(OsMountinfo {
                ts: Ts(ts),
                major: index,
                minor: 0,
                mount_point,
                root: mount_point,
                fstype: mount_point,
                source: mount_point,
                is_k8s_infra: false,
                total_bytes: Some(i64::from(index)),
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
        .append(SegmentId::new(ts).expect("segment id"), &part)
        .expect("append");
    drop(journal);
    drop(owner);

    let heatmap = super::prepare(
        directory.path(),
        crate::route::HeatmapRequest {
            from: ts,
            to: ts,
            section: "os_mountinfo".to_owned(),
            fields: vec!["total_bytes".to_owned()],
            columns: 1,
            top: 1,
            labels: Vec::new(),
            group: vec!["mount_point".to_owned()],
            type_id: None,
        },
    )
    .expect("prepare heatmap");
    let mut response = Vec::new();
    heatmap
        .stream(
            &mut |record| {
                response.extend_from_slice(&record);
                true
            },
            &|| false,
        )
        .expect("stream heatmap");
    let records: Vec<Value> = response
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("NDJSON record"))
        .collect();
    assert_eq!(records[0]["entity_count"], 129);
    assert_eq!(records[1]["identity"], json!(["/mount-128"]));
    assert_eq!(records[1]["cells"], json!([128.0]));
    assert_eq!(records[2]["band"], "totals");
    assert_eq!(records[2]["cells"], json!([8_256.0]));
    assert_eq!(records[3]["band"], "others");
    assert_eq!(records[3]["total"], json!(8_128.0));
    assert_eq!(records[3]["cells"], json!([8_128.0]));

    let ungrouped = super::prepare(
        directory.path(),
        crate::route::HeatmapRequest {
            from: ts,
            to: ts,
            section: "os_mountinfo".to_owned(),
            fields: vec!["total_bytes".to_owned()],
            columns: 1_440,
            top: 129,
            labels: vec!["fstype".to_owned()],
            group: Vec::new(),
            type_id: None,
        },
    )
    .expect("prepare ungrouped heatmap");
    let mut response = Vec::new();
    ungrouped
        .stream(
            &mut |record| {
                response.extend_from_slice(&record);
                true
            },
            &|| false,
        )
        .expect("stream ungrouped heatmap");
    let lines: Vec<&[u8]> = response
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 132, "header, every row, and two bands");
    let header: Value = serde_json::from_slice(lines[0]).expect("ungrouped header");
    let first: Value = serde_json::from_slice(lines[1]).expect("first ungrouped row");
    assert_eq!(header["top"], 129);
    assert_eq!(first["identity"], json!([128, 0, "/mount-128"]));
    assert_eq!(first["labels"], json!(["/mount-128"]));
    assert_eq!(first["cells"].as_array().map(Vec::len), Some(1_440));
    assert_eq!(first["cells"][0], json!(128.0));
    assert!(first["cells"][1].is_null());
    let totals: Value = serde_json::from_slice(lines[130]).expect("totals band");
    let others: Value = serde_json::from_slice(lines[131]).expect("others band");
    assert_eq!(totals["band"], "totals");
    assert_eq!(others["band"], "others");
}
