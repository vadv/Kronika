use serde_json::{Value, json};

use super::{Fold, Obs, column_of, entity_key_into, interval_end, interval_start};

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
    let first = ranked.totals[0].value().unwrap_or_default();
    // The second column measures from the boundary carry at minute 15, so
    // its 900 delta spreads over the 40 observed minutes.
    let second = ranked.totals[1].value().unwrap_or_default();
    assert!((first - 1.0).abs() < 1e-9);
    assert!((second - 0.375).abs() < 1e-9);
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
fn a_sparse_cadence_fills_every_later_column_through_the_boundary_carry() {
    // One sample per column, as tables and indexes record: the first column
    // has no baseline, every later one measures from the carried boundary.
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
    assert_eq!(ranked.totals[0].value(), None);
    for column in 1..12 {
        let cell = ranked.totals[column].value().unwrap_or_default();
        assert!((cell - 2.0).abs() < 1e-9, "column {column}: {cell}");
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
    let group = || Some(vec![json!("postgres")]);
    // Two workers of one command, one of them dying mid-hour, plus a loner.
    fold.observe(1, &identity("101"), group(), HOUR, Some(0.0));
    fold.observe(
        1,
        &identity("101"),
        group(),
        HOUR + 15 * MINUTE,
        Some(900.0),
    );
    fold.observe(1, &identity("102"), group(), HOUR, Some(0.0));
    fold.observe(
        1,
        &identity("102"),
        group(),
        HOUR + 40 * MINUTE,
        Some(1_200.0),
    );
    fold.observe(
        1,
        &identity("7"),
        Some(vec![json!("cron")]),
        HOUR,
        Some(0.0),
    );
    fold.observe(
        1,
        &identity("7"),
        Some(vec![json!("cron")]),
        HOUR + 40 * MINUTE,
        Some(240.0),
    );
    let grouped = fold.finish_grouped(1);
    assert_eq!(grouped.group_count, 2);
    assert_eq!(grouped.rows.len(), 1);
    let top = &grouped.rows[0];
    assert_eq!(top.values, vec![json!("postgres")]);
    assert_eq!(top.members, 2);
    assert_eq!(top.total, Some(2_100.0));
    let first = top.cells[0].unwrap_or_default();
    assert!(first > 0.0);
    assert_eq!(grouped.others_total, Some(240.0));
    // cron's first column has one sample and no baseline; its delta lands in
    // the second column through the carry.
    assert_eq!(grouped.others[0].value(), None);
    let others_second = grouped.others[1].value().unwrap_or_default();
    assert!(others_second > 0.0);
}
