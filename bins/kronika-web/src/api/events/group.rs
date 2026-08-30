//! Exact grouping rules shared by the Events transports.

use std::collections::{HashMap, HashSet};

use icu_collator::options::{AlternateHandling, CaseLevel, CollatorOptions, Strength};
use icu_collator::{Collator, CollatorBorrowed};
use icu_locale_core::locale;
use serde_json::{Value, json};

use super::{
    EventDataRow, EventGroup, EventSource, EventStat, EventTier, MINUTE_COLUMNS, MINUTE_MICROS,
    StoredEventRow,
};
use crate::api::ApiError;

pub(super) fn group_events(
    mut streams: HashMap<EventSource, Vec<EventDataRow>>,
    from: i64,
    threshold_ms: Option<f64>,
) -> Result<Vec<EventGroup>, ApiError> {
    let mut entries = Vec::new();
    entries.extend(group_errors(
        streams.remove(&EventSource::Errors).unwrap_or_default(),
        from,
    ));
    entries.extend(group_slow(
        streams
            .remove(&EventSource::SlowQueries)
            .unwrap_or_default(),
        from,
        threshold_ms,
    ));
    entries.extend(group_autovacuum(
        streams.remove(&EventSource::Autovacuum).unwrap_or_default(),
        from,
    ));
    entries.extend(group_checkpoints(
        streams
            .remove(&EventSource::Checkpoints)
            .unwrap_or_default(),
        from,
    ));
    entries.extend(group_locks(
        streams.remove(&EventSource::LockWaits).unwrap_or_default(),
        from,
    ));
    entries.extend(group_lifecycle(
        streams.remove(&EventSource::Lifecycle).unwrap_or_default(),
        from,
    ));
    entries.extend(group_pgbouncer(
        streams.remove(&EventSource::Pgbouncer).unwrap_or_default(),
        from,
    ));

    let collator = event_collator()?;
    entries.sort_by(|left, right| {
        tier_order(left.tier)
            .cmp(&tier_order(right.tier))
            .then_with(|| right.count.total_cmp(&left.count))
            .then_with(|| right.last_ts.cmp(&left.last_ts))
            .then_with(|| collator.compare(&left.key, &right.key))
    });
    for entry in &mut entries {
        if let EventStat::Pgbouncer { level, .. } = &entry.stat {
            let digest = entry
                .detail_locator
                .row_key
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or("missing");
            entry.key = format!("pgbouncer:{level}:{digest}");
        }
    }
    Ok(entries)
}

pub(super) fn event_collator() -> Result<CollatorBorrowed<'static>, ApiError> {
    let mut options = CollatorOptions::default();
    options.strength = Some(Strength::Tertiary);
    options.alternate_handling = Some(AlternateHandling::NonIgnorable);
    options.case_level = Some(CaseLevel::Off);
    Collator::try_new(locale!("en-US").into(), options).map_err(|error| {
        ApiError::Unreadable(Box::new(std::io::Error::other(format!(
            "create en-US event-key collator: {error}"
        ))))
    })
}

fn group_errors(rows: Vec<EventDataRow>, from: i64) -> Vec<EventGroup> {
    grouped(rows, |row| {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            text(row, "severity").unwrap_or_default(),
            text(row, "category").unwrap_or_default(),
            text(row, "pattern").unwrap_or_default()
        )
    })
    .into_iter()
    .map(|(key, members)| {
        let first = &members[0];
        let weights: Vec<f64> = members
            .iter()
            .map(|row| number(row, "count").unwrap_or(1.0))
            .collect();
        let severity = number(first, "severity").unwrap_or(0.0);
        let label = text(first, "pattern");
        let category = number(first, "category");
        let sqlstate = text(first, "sqlstate");
        let database = shared(&members, "database");
        let username = shared(&members, "username");
        build(
            format!("errors:{key}"),
            EventSource::Errors,
            &members,
            from,
            Some(&weights),
            None,
            error_tier(severity),
            label,
            EventStat::Errors {
                severity,
                category,
                sqlstate,
                database,
                username,
            },
        )
    })
    .collect()
}

fn group_slow(rows: Vec<EventDataRow>, from: i64, threshold_ms: Option<f64>) -> Vec<EventGroup> {
    grouped(rows, |row| text(row, "pattern").unwrap_or_default())
        .into_iter()
        .map(|(key, members)| {
            let first = &members[0];
            let weights: Vec<f64> = members
                .iter()
                .map(|row| number(row, "count").unwrap_or(1.0))
                .collect();
            let slowest = members.iter().skip(1).fold(first, |chosen, row| {
                if number(row, "max_duration_ms").unwrap_or(0.0)
                    > number(chosen, "max_duration_ms").unwrap_or(0.0)
                {
                    row
                } else {
                    chosen
                }
            });
            let max_ms = number(slowest, "max_duration_ms").unwrap_or(0.0);
            let total_ms = sum(&members, "total_duration_ms").unwrap_or(0.0);
            let representative = super::row_key::detail_locator(
                EventSource::SlowQueries.as_str(),
                slowest.segment_id,
                slowest.timestamp,
                slowest.type_id,
                slowest.row_ordinal,
                &slowest.values,
            );
            let mut group = build(
                format!("slow:{key}"),
                EventSource::SlowQueries,
                &members,
                from,
                Some(&weights),
                None,
                EventTier::Notable,
                Some(key),
                EventStat::Slow {
                    max_ms,
                    total_ms,
                    threshold_ms,
                },
            );
            group.detail_locator = representative;
            group
        })
        .collect()
}

fn group_autovacuum(rows: Vec<EventDataRow>, from: i64) -> Vec<EventGroup> {
    grouped(rows, |row| {
        format!(
            "{}\u{1f}{}",
            text(row, "kind").unwrap_or_default(),
            text(row, "relation").unwrap_or_default()
        )
    })
    .into_iter()
    .map(|(key, members)| {
        let first = &members[0];
        let last = &members[members.len() - 1];
        let label = text(first, "relation");
        let analyze = number(first, "kind") == Some(1.0);
        let runs = members.len();
        let total_ms = sum(&members, "elapsed_ms");
        let tuples_removed = sum(&members, "tuples_removed");
        let tuples_dead = number(last, "tuples_dead_not_removable");
        build(
            format!("autovacuum:{key}"),
            EventSource::Autovacuum,
            &members,
            from,
            None,
            None,
            EventTier::Routine,
            label,
            EventStat::Autovacuum {
                analyze,
                runs,
                total_ms,
                tuples_removed,
                tuples_dead,
            },
        )
    })
    .collect()
}

fn group_checkpoints(rows: Vec<EventDataRow>, from: i64) -> Vec<EventGroup> {
    let (warnings, ordinary): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| number(row, "phase") == Some(2.0));
    let mut entries = Vec::new();
    if !ordinary.is_empty() {
        let starts = ordinary
            .iter()
            .filter(|row| number(row, "phase") == Some(0.0))
            .count();
        let completes = ordinary
            .iter()
            .filter(|row| number(row, "phase") == Some(1.0))
            .count();
        let timed = ordinary
            .iter()
            .filter(|row| {
                number(row, "phase") == Some(0.0)
                    && text(row, "reason").is_some_and(|reason| reason.contains("time"))
            })
            .count();
        let count = starts.max(completes);
        let max_sync_ms = ordinary
            .iter()
            .filter(|row| number(row, "phase") == Some(1.0))
            .filter_map(|row| number(row, "sync_ms"))
            .max_by(f64::total_cmp);
        let mut buffer_values = ordinary
            .iter()
            .filter(|row| number(row, "phase") == Some(1.0))
            .filter_map(|row| number(row, "buffers_written"))
            .peekable();
        let buffers = buffer_values.peek().is_some().then(|| buffer_values.sum());
        entries.push(build(
            "checkpoints".to_owned(),
            EventSource::Checkpoints,
            &ordinary,
            from,
            None,
            Some(count_number(count)),
            EventTier::Routine,
            None,
            EventStat::Checkpoints {
                completes,
                timed,
                requested: starts - timed,
                max_sync_ms,
                buffers,
            },
        ));
    }
    if !warnings.is_empty() {
        let seconds_apart = min(&warnings, "seconds_apart");
        entries.push(build(
            "checkpoints:warning".to_owned(),
            EventSource::Checkpoints,
            &warnings,
            from,
            None,
            None,
            EventTier::Notable,
            None,
            EventStat::CheckpointWarning { seconds_apart },
        ));
    }
    entries
}

fn group_locks(rows: Vec<EventDataRow>, from: i64) -> Vec<EventGroup> {
    let (waits, acquired): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| number(row, "kind") == Some(0.0));
    let episodes = grouped(waits, |row| text(row, "holding_pids").unwrap_or_default());
    let mut waiter_of = HashMap::new();
    for (key, members) in &episodes {
        for row in members {
            waiter_of.insert(
                format!(
                    "{}\u{1f}{}",
                    text(row, "pid").unwrap_or_default(),
                    text(row, "lock_target").unwrap_or_default()
                ),
                key.clone(),
            );
        }
    }
    let mut attached: HashMap<String, Vec<EventDataRow>> = HashMap::new();
    let mut leftovers = Vec::new();
    for row in acquired {
        let join = format!(
            "{}\u{1f}{}",
            text(&row, "pid").unwrap_or_default(),
            text(&row, "lock_target").unwrap_or_default()
        );
        if let Some(key) = waiter_of.get(&join) {
            attached.entry(key.clone()).or_default().push(row);
        } else {
            leftovers.push(row);
        }
    }
    let mut entries = episodes
        .into_iter()
        .map(|(key, mut members)| {
            let waits = members.len();
            members.extend(attached.remove(&key).unwrap_or_default());
            members.sort_by_key(|row| row.timestamp);
            lock_group(key, waits, false, &members, from)
        })
        .collect::<Vec<_>>();
    if !leftovers.is_empty() {
        let waits = leftovers.len();
        entries.push(lock_group(String::new(), waits, true, &leftovers, from));
    }
    entries
}

fn lock_group(
    key: String,
    waits: usize,
    acquired: bool,
    members: &[EventDataRow],
    from: i64,
) -> EventGroup {
    let waiters = members
        .iter()
        .map(|row| text(row, "pid").unwrap_or_default())
        .collect::<HashSet<_>>()
        .len();
    let mut targets = Vec::new();
    let mut seen_targets = HashSet::new();
    for target in members.iter().filter_map(|row| text(row, "lock_target")) {
        if !target.is_empty() && seen_targets.insert(target.clone()) {
            targets.push(target);
        }
    }
    let max_ms = max(members, "duration_ms");
    build(
        format!("locks:{}", if acquired { "acquired" } else { &key }),
        EventSource::LockWaits,
        members,
        from,
        None,
        Some(count_number(waits.max(1))),
        EventTier::Notable,
        None,
        EventStat::Locks {
            holders: (!key.is_empty()).then_some(key),
            acquired,
            waiters,
            max_ms,
            targets,
        },
    )
}

fn group_lifecycle(rows: Vec<EventDataRow>, from: i64) -> Vec<EventGroup> {
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let lifecycle = number(&row, "kind").unwrap_or(0.0);
            let pid = number(&row, "pid");
            let signal = number(&row, "signal");
            let mode = text(&row, "shutdown_mode");
            build(
                format!("lifecycle:{index}:{}", row.row_ordinal),
                EventSource::Lifecycle,
                std::slice::from_ref(&row),
                from,
                None,
                None,
                lifecycle_tier(lifecycle),
                None,
                EventStat::Lifecycle {
                    lifecycle,
                    pid,
                    signal,
                    mode,
                },
            )
        })
        .collect()
}

fn group_pgbouncer(rows: Vec<EventDataRow>, from: i64) -> Vec<EventGroup> {
    grouped(rows, |row| {
        format!(
            "{}\u{1f}{}",
            text(row, "level").unwrap_or_default(),
            text(row, "text").unwrap_or_default()
        )
    })
    .into_iter()
    .map(|(key, members)| {
        let first = &members[0];
        let level = number(first, "level").unwrap_or(3.0);
        let database = shared(&members, "database");
        build(
            format!("pgbouncer:{key}"),
            EventSource::Pgbouncer,
            &members,
            from,
            None,
            None,
            pgbouncer_tier(level),
            None,
            EventStat::Pgbouncer { level, database },
        )
    })
    .collect()
}

fn grouped(
    mut rows: Vec<EventDataRow>,
    key_of: impl Fn(&EventDataRow) -> String,
) -> Vec<(String, Vec<EventDataRow>)> {
    rows.sort_by_key(|row| row.timestamp);
    let mut groups: Vec<(String, Vec<EventDataRow>)> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    for row in rows {
        let key = key_of(&row);
        if let Some(index) = positions.get(&key).copied() {
            groups[index].1.push(row);
        } else {
            positions.insert(key.clone(), groups.len());
            groups.push((key, vec![row]));
        }
    }
    groups
}

#[expect(
    clippy::too_many_arguments,
    reason = "the argument list mirrors the deliberately flat EventEntry contract"
)]
fn build(
    key: String,
    source: EventSource,
    members: &[EventDataRow],
    from: i64,
    weights: Option<&[f64]>,
    count: Option<f64>,
    tier: EventTier,
    label: Option<String>,
    stat: EventStat,
) -> EventGroup {
    let representative = &members[0];
    let detail_locator = super::row_key::detail_locator(
        source.as_str(),
        representative.segment_id,
        representative.timestamp,
        representative.type_id,
        representative.row_ordinal,
        &representative.values,
    );
    let mut minutes = vec![0.0; MINUTE_COLUMNS];
    let mut first_ts = i64::MAX;
    let mut last_ts = i64::MIN;
    for (index, row) in members.iter().enumerate() {
        if let Some(delta) = row.timestamp.checked_sub(from) {
            let bucket = delta.div_euclid(MINUTE_MICROS);
            if let Ok(bucket) = usize::try_from(bucket)
                && let Some(slot) = minutes.get_mut(bucket)
            {
                *slot += weights
                    .and_then(|weights| weights.get(index))
                    .copied()
                    .unwrap_or(1.0);
            }
        }
        first_ts = first_ts.min(row.timestamp);
        last_ts = last_ts.max(row.timestamp);
    }
    EventGroup {
        key,
        section: source.as_str().to_owned(),
        tier,
        label,
        count: count.unwrap_or_else(|| {
            weights.map_or_else(
                || count_number(members.len()),
                |weights| weights.iter().sum(),
            )
        }),
        first_ts,
        last_ts,
        minutes,
        stat,
        detail_locator,
    }
}

fn text(row: &EventDataRow, field: &str) -> Option<String> {
    raw_text(row.values.get(field).unwrap_or(&Value::Null))
}

fn raw_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(object) if object.get("representation") == Some(&json!("text")) => object
            .get("stored_text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::Object(object) if object.get("representation") == Some(&json!("bytes")) => object
            .get("stored_base64")
            .or_else(|| object.get("base64"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        value => Some(value.to_string()),
    }
}

fn number(row: &EventDataRow, field: &str) -> Option<f64> {
    let value = row.values.get(field)?;
    let parsed = match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) if !value.trim().is_empty() => value.parse().ok(),
        _ => None,
    }?;
    parsed.is_finite().then_some(parsed)
}

fn shared(members: &[EventDataRow], field: &str) -> Option<String> {
    let (first, rest) = members.split_first()?;
    let shared = text(first, field).unwrap_or_default();
    if shared.is_empty() {
        return None;
    }
    rest.iter()
        .all(|row| text(row, field).unwrap_or_default() == shared)
        .then_some(shared)
}

fn sum(members: &[EventDataRow], field: &str) -> Option<f64> {
    let mut values = members
        .iter()
        .filter_map(|row| number(row, field))
        .peekable();
    values.peek()?;
    Some(values.sum())
}

fn max(members: &[EventDataRow], field: &str) -> Option<f64> {
    members
        .iter()
        .filter_map(|row| number(row, field))
        .max_by(f64::total_cmp)
}

fn min(members: &[EventDataRow], field: &str) -> Option<f64> {
    members
        .iter()
        .filter_map(|row| number(row, field))
        .min_by(f64::total_cmp)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the wire contract is a JavaScript number and event counts use that representation"
)]
const fn count_number(value: usize) -> f64 {
    value as f64
}

pub(super) fn slow_threshold_ms(rows: &[StoredEventRow]) -> Option<f64> {
    let row = rows
        .iter()
        .filter(|row| {
            row.fields.get("name").and_then(raw_text).as_deref()
                == Some("log_min_duration_statement")
        })
        .reduce(|chosen, row| if row.at > chosen.at { row } else { chosen })?;
    let setting = row
        .fields
        .get("setting")
        .and_then(raw_text)?
        .parse::<f64>()
        .ok()?;
    if !setting.is_finite() || setting < 0.0 {
        return None;
    }
    Some(match row.fields.get("unit").and_then(raw_text).as_deref() {
        Some("s") => setting * 1_000.0,
        Some("min") => setting * 60_000.0,
        _ => setting,
    })
}

const fn tier_order(tier: EventTier) -> u8 {
    match tier {
        EventTier::Critical => 0,
        EventTier::Notable => 1,
        EventTier::Routine => 2,
    }
}

fn error_tier(code: f64) -> EventTier {
    match code {
        1.0 | 2.0 => EventTier::Critical,
        3.0 | 4.0 => EventTier::Routine,
        _ => EventTier::Notable,
    }
}

fn lifecycle_tier(code: f64) -> EventTier {
    match code {
        0.0 => EventTier::Critical,
        _ => EventTier::Notable,
    }
}

fn pgbouncer_tier(code: f64) -> EventTier {
    match code {
        0.0 => EventTier::Critical,
        1.0 | 2.0 => EventTier::Notable,
        _ => EventTier::Routine,
    }
}
